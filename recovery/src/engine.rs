//! 恢复引擎：有界重试 + 指数退避 + 升级人工。

use crate::classify::FailureRecord;
use forge_core::ForgeResult;
use forge_event::{Event, EventBus, Topic};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// 恢复动作。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// 重试，附带退避毫秒数。
    Retry { backoff_ms: u64 },
    /// 跳过（非关键步骤，第一阶段保留变体）。
    Skip,
    /// 升级到人工处理。
    EscalateHuman,
}

/// 恢复策略 trait。
pub trait RecoveryStrategy: Send + Sync {
    /// 决策恢复动作。
    fn decide(&self, record: &FailureRecord, attempts: u32) -> RecoveryAction;
}

/// 有界重试策略。
///
/// - `retriable=false` → `EscalateHuman`
/// - `attempts < max_attempts` → `Retry { base * 2^attempts }`
/// - 否则 → `EscalateHuman`
pub struct BoundedRetryStrategy {
    /// 最大重试次数，默认 3。
    pub max_attempts: u32,
    /// 基础退避毫秒，默认 1000。
    pub base_backoff_ms: u64,
}

impl Default for BoundedRetryStrategy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_backoff_ms: 1000,
        }
    }
}

/// 单次退避上限（毫秒）。
const MAX_BACKOFF_MS: u64 = 60_000;

impl RecoveryStrategy for BoundedRetryStrategy {
    fn decide(&self, record: &FailureRecord, attempts: u32) -> RecoveryAction {
        if !record.retriable {
            return RecoveryAction::EscalateHuman;
        }
        if attempts < self.max_attempts {
            let backoff = self.base_backoff_ms.saturating_mul(2u64.saturating_pow(attempts));
            let backoff = backoff.min(MAX_BACKOFF_MS);
            RecoveryAction::Retry { backoff_ms: backoff }
        } else {
            RecoveryAction::EscalateHuman
        }
    }
}

/// 恢复引擎。
pub struct RecoveryEngine {
    strategy: Arc<dyn RecoveryStrategy>,
    event_bus: Arc<dyn EventBus>,
}

impl RecoveryEngine {
    /// 创建恢复引擎。
    pub fn new(strategy: Arc<dyn RecoveryStrategy>, event_bus: Arc<dyn EventBus>) -> Self {
        Self { strategy, event_bus }
    }

    /// 处理失败记录，决策恢复动作并广播事件。
    pub async fn handle(
        &self,
        record: FailureRecord,
        attempts: u32,
    ) -> ForgeResult<RecoveryAction> {
        let action = self.strategy.decide(&record, attempts);
        info!(
            execution_id = %record.execution_id,
            category = ?record.category,
            retriable = record.retriable,
            attempts,
            ?action,
            "recovery decision made"
        );

        // 发布恢复事件
        let event = Event::new(
            Topic::Recovery,
            serde_json::json!({
                "category": format!("{:?}", record.category),
                "action": format!("{:?}", action),
                "execution_id": record.execution_id.to_string(),
                "attempts": attempts,
            }),
        );
        self.event_bus.publish(event).await?;

        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{classify, FailureCategory};
    use forge_core::ExecutionId;
    use forge_event::InMemoryEventBus;
    use forge_exec::ExecutionStatus;

    #[tokio::test]
    async fn test_retry_backoff() {
        let strategy = BoundedRetryStrategy::default();
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::Failed, "error").unwrap();

        // attempts 0 → 1000ms
        assert_eq!(
            strategy.decide(&record, 0),
            RecoveryAction::Retry { backoff_ms: 1000 }
        );
        // attempts 1 → 2000ms
        assert_eq!(
            strategy.decide(&record, 1),
            RecoveryAction::Retry { backoff_ms: 2000 }
        );
        // attempts 2 → 4000ms
        assert_eq!(
            strategy.decide(&record, 2),
            RecoveryAction::Retry { backoff_ms: 4000 }
        );
        // attempts 3 → EscalateHuman
        assert_eq!(
            strategy.decide(&record, 3),
            RecoveryAction::EscalateHuman
        );
    }

    #[tokio::test]
    async fn test_non_retriable_escalates() {
        let strategy = BoundedRetryStrategy::default();
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::PermissionDenied, "denied").unwrap();

        assert_eq!(
            strategy.decide(&record, 0),
            RecoveryAction::EscalateHuman
        );
    }

    #[tokio::test]
    async fn test_engine_publishes_event() {
        let strategy: Arc<dyn RecoveryStrategy> = Arc::new(BoundedRetryStrategy::default());
        let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

        // 先订阅 Recovery topic
        let mut rx = event_bus.subscribe(Topic::Recovery).await.unwrap();

        let engine = RecoveryEngine::new(strategy, event_bus);
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::Failed, "error").unwrap();

        let action = engine.handle(record, 0).await.unwrap();
        assert!(matches!(action, RecoveryAction::Retry { .. }));

        // 应收到恢复事件
        let event = rx.recv().await.unwrap();
        assert_eq!(event.topic, Topic::Recovery);
        assert!(event.payload["action"].as_str().unwrap().contains("Retry"));
    }

    #[tokio::test]
    async fn test_backoff_cap() {
        let strategy = BoundedRetryStrategy {
            max_attempts: 100,
            base_backoff_ms: 100_000, // 大基础值
        };
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::Failed, "error").unwrap();

        let action = strategy.decide(&record, 0);
        match action {
            RecoveryAction::Retry { backoff_ms } => {
                assert!(backoff_ms <= MAX_BACKOFF_MS, "backoff should be capped at 60000, got {}", backoff_ms);
            }
            _ => panic!("expected Retry"),
        }
    }
}
