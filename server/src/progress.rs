//! 执行进度流（V8.0 STREAM-001）。
//!
//! [`BusProgressStore`]：装饰 [`forge_session::SessionStore`]，`append` 成功后向本地
//! 事件总线发布一条进度事件（失败仅 warn，不阻断会话记录）。进度载荷字段冻结。
//!
//! 契约来源：build_v80a.md AF-BP-V80A STREAM-001。

use async_trait::async_trait;
use forge_core::{ForgeResult, SessionId, TaskId};
use forge_event::{Event, EventBus, Topic};
use forge_session::{Session, SessionEvent, SessionEventKind, SessionStore};
use std::sync::Arc;

/// 会话存储装饰器：append 成功后向事件总线发布进度事件（失败仅 warn）。
pub struct BusProgressStore {
    inner: Arc<dyn SessionStore>,
    bus: Arc<dyn EventBus>,
}

impl BusProgressStore {
    pub fn new(inner: Arc<dyn SessionStore>, bus: Arc<dyn EventBus>) -> Self {
        Self { inner, bus }
    }

    /// 从事件种类映射进度状态（工作台展示用）。
    fn status_of(kind: &SessionEventKind) -> &'static str {
        match kind {
            SessionEventKind::TaskReceived => "received",
            SessionEventKind::PlanCreated => "planned",
            SessionEventKind::ActionDispatched => "dispatched",
            SessionEventKind::ActionResult => "result",
            SessionEventKind::VerificationResult => "verified",
            SessionEventKind::Failed => "failed",
            SessionEventKind::Recovered => "recovered",
            SessionEventKind::Completed => "completed",
        }
    }

    /// 组装冻结进度载荷：{"session_id","kind","step"?,"status","seq","at"}。
    fn progress_payload(id: &SessionId, event: &SessionEvent) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("session_id".into(), serde_json::json!(id.to_string()));
        m.insert(
            "kind".into(),
            serde_json::to_value(&event.kind).unwrap_or(serde_json::Value::Null),
        );
        m.insert("status".into(), serde_json::json!(Self::status_of(&event.kind)));
        m.insert("seq".into(), serde_json::json!(event.seq));
        m.insert("at".into(), serde_json::json!(event.at.to_rfc3339()));
        // step?：从 payload 提取 step_id / step（若存在）。
        if let Some(step) = event.payload.get("step_id").or_else(|| event.payload.get("step")) {
            m.insert("step".into(), step.clone());
        }
        serde_json::Value::Object(m)
    }
}

#[async_trait]
impl SessionStore for BusProgressStore {
    async fn create(&self, task_id: TaskId) -> ForgeResult<Session> {
        self.inner.create(task_id).await
    }

    async fn append(
        &self,
        id: &SessionId,
        kind: SessionEventKind,
        payload: serde_json::Value,
    ) -> ForgeResult<SessionEvent> {
        let event = self.inner.append(id, kind, payload).await?;
        // 发布进度事件到 Session 主题；失败仅 warn，不阻断会话记录。
        let progress = Self::progress_payload(id, &event);
        if let Err(e) = self.bus.publish(Event::new(Topic::Session, progress)).await {
            tracing::warn!(session_id = %id, error = %e, "progress publish failed (non-blocking)");
        }
        Ok(event)
    }

    async fn get(&self, id: &SessionId) -> ForgeResult<Session> {
        self.inner.get(id).await
    }

    async fn list(&self) -> ForgeResult<Vec<SessionId>> {
        self.inner.list().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_session::InMemorySessionStore;

    #[tokio::test]
    async fn progress_events_published_on_append() {
        let raw: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let bus: Arc<dyn EventBus> = Arc::new(forge_event::InMemoryEventBus::with_buffer(16));
        let store: Arc<dyn SessionStore> = Arc::new(BusProgressStore::new(raw, bus.clone()));

        // 先订阅、后触发 append——EventBus 契约"订阅前发布的事件不回放"
        //（tokio broadcast 语义），顺序颠倒会永久等不到而挂起（已修，并加超时护栏）。
        let mut stream = bus.subscribe(Topic::Session).await.unwrap();

        let task = TaskId::from("T-1".to_string());
        let session = store.create(task).await.unwrap();
        store
            .append(
                &session.id,
                SessionEventKind::ActionDispatched,
                serde_json::json!({"tool": "echo", "step_id": "s1#1"}),
            )
            .await
            .unwrap();

        // 订阅 spy 应收到载荷，字段冻结；超时护栏防止回归以"挂起"形态出现。
        let evt = tokio::time::timeout(std::time::Duration::from_secs(5), stream.recv())
            .await
            .expect("timeout waiting for progress event — 订阅/发布时序回归")
            .expect("progress event stream closed");
        let p = evt.payload;
        assert_eq!(p["session_id"], session.id.to_string());
        assert_eq!(p["kind"], "ActionDispatched");
        assert_eq!(p["status"], "dispatched");
        assert_eq!(p["step"], "s1#1");
        assert_eq!(p["seq"], 1);
        assert!(p["at"].is_string());
    }
}