//! TEN-003: 租户配额与限流
//!
//! 按租户限制并发编排数与日任务量。

use async_trait::async_trait;
use forge_core::ForgeResult;
use serde::{Deserialize, Serialize};

/// 配额视图
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuotaView {
    pub max_concurrent: i32,
    pub daily_tasks: i32,
}

/// 配额存储 trait
#[async_trait]
pub trait QuotaStore: Send + Sync {
    /// 获取租户配额（无记录则返回默认值）
    async fn of(&self, tenant_id: &str) -> ForgeResult<QuotaView>;
    /// 设置配额（仅管理员）
    async fn set(&self, tenant_id: &str, q: QuotaView) -> ForgeResult<()>;
}

/// 检查配额是否超标
pub async fn check_quota(q: &QuotaView, running: i64, today_count: i64) -> ForgeResult<()> {
    if running >= q.max_concurrent as i64 {
        return Err(forge_core::ForgeError::InvalidState(
            format!("quota_concurrency: max_concurrent={} reached", q.max_concurrent)
        ));
    }
    if today_count >= q.daily_tasks as i64 {
        return Err(forge_core::ForgeError::InvalidState(
            format!("quota_daily: daily_tasks={} reached", q.daily_tasks)
        ));
    }
    Ok(())
}

/// 内存实现（测试用）
pub struct InMemoryQuotaStore {
    quotas: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, QuotaView>>>,
}

impl Default for InMemoryQuotaStore {
    fn default() -> Self {
        Self {
            quotas: std::sync::Arc::new(std::sync::RwLock::new(
                [(String::from("default"), QuotaView::default())].into_iter().collect()
            )),
        }
    }
}

#[async_trait]
impl QuotaStore for InMemoryQuotaStore {
    async fn of(&self, tenant_id: &str) -> ForgeResult<QuotaView> {
        let guard = self.quotas.read().unwrap();
        Ok(guard.get(tenant_id).cloned().unwrap_or_default())
    }
    
    async fn set(&self, tenant_id: &str, q: QuotaView) -> ForgeResult<()> {
        let mut guard = self.quotas.write().unwrap();
        guard.insert(tenant_id.to_string(), q);
        Ok(())
    }
}
