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
        return Err(ForgeError::QuotaExceeded(format!(
            "quota_daily: tenant has {} tasks today, limit {}",
            today_count, q.daily_tasks
        )));
    }
    Ok(())
}
