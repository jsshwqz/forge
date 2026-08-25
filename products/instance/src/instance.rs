//! 产品实例生命周期（PROD-001）。
//!
//! 状态机（契约 6.1）：`Draft → Active ⇄ Stopped → Deprecated`；
//! 非法迁移一律 `InvalidState`。存储走 trait，内存实现供测试/单机，
//! 后续 PG 实现按 forge-storage 模式补齐。

use chrono::{DateTime, Utc};
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 实例状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductState {
    /// 已创建未启动。
    Draft,
    /// 运行中。
    Active,
    /// 已停止（可重新 start）。
    Stopped,
    /// 已弃用（终态）。
    Deprecated,
}

/// 产品实例。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductInstance {
    /// 实例 ID（`pinst_` 前缀）。
    pub id: String,
    /// 来源模板 ID。
    pub template_id: String,
    /// 来源模板版本。
    pub template_version: String,
    /// 实例名称。
    pub name: String,
    /// 实例化参数。
    pub params: HashMap<String, String>,
    /// 当前状态。
    pub state: ProductState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProductInstance {
    /// 状态迁移（唯一合法路径写在这里）。
    pub fn transition(&mut self, to: ProductState) -> ForgeResult<()> {
        let legal = matches!(
            (self.state, to),
            (ProductState::Draft, ProductState::Active)
                | (ProductState::Active, ProductState::Stopped)
                | (ProductState::Stopped, ProductState::Active)
                | (ProductState::Stopped, ProductState::Deprecated)
                | (ProductState::Draft, ProductState::Deprecated)
        );
        if !legal {
            return Err(ForgeError::InvalidState(format!(
                "product instance {}: illegal transition {:?} -> {:?}",
                self.id, self.state, to
            )));
        }
        self.state = to;
        self.updated_at = Utc::now();
        Ok(())
    }
}

/// 实例存储 trait。
#[async_trait::async_trait]
pub trait ProductInstanceStore: Send + Sync {
    async fn insert(&self, inst: ProductInstance) -> ForgeResult<()>;
    async fn get(&self, id: &str) -> ForgeResult<ProductInstance>;
    async fn list(&self) -> ForgeResult<Vec<ProductInstance>>;
    async fn update(&self, inst: ProductInstance) -> ForgeResult<()>;
}

/// 内存实现。
#[derive(Default)]
pub struct InMemoryProductInstanceStore {
    inner: Arc<RwLock<HashMap<String, ProductInstance>>>,
}

impl InMemoryProductInstanceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ProductInstanceStore for InMemoryProductInstanceStore {
    async fn insert(&self, inst: ProductInstance) -> ForgeResult<()> {
        let mut g = self.inner.write().await;
        if g.contains_key(&inst.id) {
            return Err(ForgeError::InvalidState(format!(
                "product instance {} already exists",
                inst.id
            )));
        }
        g.insert(inst.id.clone(), inst);
        Ok(())
    }

    async fn get(&self, id: &str) -> ForgeResult<ProductInstance> {
        self.inner
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("product instance: {id}")))
    }

    async fn list(&self) -> ForgeResult<Vec<ProductInstance>> {
        let mut v: Vec<_> = self.inner.read().await.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(v)
    }

    async fn update(&self, inst: ProductInstance) -> ForgeResult<()> {
        let mut g = self.inner.write().await;
        if !g.contains_key(&inst.id) {
            return Err(ForgeError::NotFound(format!("product instance: {}", inst.id)));
        }
        g.insert(inst.id.clone(), inst);
        Ok(())
    }
}

/// 新实例 ID。
pub fn new_product_instance_id() -> String {
    format!("pinst_{}", uuid_lite())
}

/// 轻量 uuid v4（复用 forge-core 的 id 生成器风格，避免新依赖）。
fn uuid_lite() -> String {
    // forge-core 未导出裸 uuid；此处用时间+计数足够满足单机唯一性
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let t = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{t:x}{n:04x}")
}
