//! V5.0 TEN-003: 租户配额与限流
//!
//! 按租户限制并发编排与日任务量。

use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};

/// 租户配额视图
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuotaView {
    pub max_concurrent: i32,
    pub daily_tasks: i32,
}

/// 配额存储 trait
#[async_trait::async_trait]
pub trait QuotaStore: Send + Sync {
    /// 获取租户配额，无记录返回默认值 (4, 100)
    async fn of(&self, tenant_id: &str) -> ForgeResult<QuotaView>;
    /// 设置租户配额
    async fn set(&self, tenant_id: &str, q: QuotaView) -> ForgeResult<()>;
}

/// 检查配额是否超标
/// 超并发 → 429 quota_concurrency
/// 超日量 → 429 quota_daily
pub async fn check_quota(q: &QuotaView, running: i64, today_count: i64) -> ForgeResult<()> {
    if running >= q.max_concurrent as i64 {
        return Err(ForgeError::InvalidState(format!(
            "quota_concurrency: tenant has {} running, limit {}",
            running, q.max_concurrent
        )));
    }
    if today_count >= q.daily_tasks as i64 {
        return Err(ForgeError::InvalidState(format!(
            "quota_daily: tenant has {} tasks today, limit {}",
            today_count, q.daily_tasks
        )));
    }
    Ok(())
}

/// 默认配额（TEN-003 R1）：并发 4、日任务 100（与 0011_quotas 种子数据一致）。
pub const DEFAULT_QUOTA: QuotaView = QuotaView { max_concurrent: 4, daily_tasks: 100 };

/// 内存配额存储（开发/测试用；生产走 0011_quotas 的 PG 实现）。
#[derive(Default)]
pub struct InMemoryQuotaStore {
    quotas: tokio::sync::RwLock<std::collections::HashMap<String, QuotaView>>,
}

#[async_trait::async_trait]
impl QuotaStore for InMemoryQuotaStore {
    async fn of(&self, tenant_id: &str) -> ForgeResult<QuotaView> {
        Ok(self
            .quotas
            .read()
            .await
            .get(tenant_id)
            .cloned()
            .unwrap_or(DEFAULT_QUOTA))
    }

    async fn set(&self, tenant_id: &str, q: QuotaView) -> ForgeResult<()> {
        self.quotas
            .write()
            .await
            .insert(tenant_id.to_string(), q);
        Ok(())
    }
}
