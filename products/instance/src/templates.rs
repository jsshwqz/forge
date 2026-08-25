//! 模板库管理（PROD-002）：注册 / 列举 / 版本化；发布需 Reviewer verdict。
//!
//! 规则（契约 6.1，衔接 V3.2）：
//! - `publish` 必须携带 Reviewer verdict（Pass 或 Concern 才可入库；
//!   Reject 与缺失均拒绝——Reject 语义上应回炉，不进模板库）；
//! - 同 `id + version` 重复发布 → `InvalidState`；
//! - 版本字符串不做 semver 强校验（与 forge-cap 一致，仅非空）。

use forge_core::{ForgeError, ForgeResult};
use forge_product::ProductTemplate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 一条已发布的模板记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateRecord {
    pub template: ProductTemplate,
    /// semver 风格版本串（非空即可）。
    pub version: String,
    /// 发布时的 Reviewer 裁决（"Pass" | "Concern"）。
    pub review_verdict: String,
    pub published_at: chrono::DateTime<chrono::Utc>,
}

/// 模板库 trait。
#[async_trait::async_trait]
pub trait TemplateRegistry: Send + Sync {
    async fn publish(&self, rec: TemplateRecord) -> ForgeResult<()>;
    async fn get(&self, id: &str, version: &str) -> ForgeResult<TemplateRecord>;
    /// 某模板的全部版本（按 version 字典序）。
    async fn versions(&self, id: &str) -> ForgeResult<Vec<TemplateRecord>>;
    async fn list(&self) -> ForgeResult<Vec<TemplateRecord>>;
}

fn validate_verdict(v: &str) -> ForgeResult<()> {
    match v {
        "Pass" | "Concern" => Ok(()),
        other => Err(ForgeError::InvalidState(format!(
            "template publish: review verdict must be Pass or Concern, got '{other}'"
        ))),
    }
}

fn validate_rec(rec: &TemplateRecord) -> ForgeResult<()> {
    validate_verdict(&rec.review_verdict)?;
    if rec.version.trim().is_empty() {
        return Err(ForgeError::InvalidState("template publish: empty version".into()));
    }
    if rec.template.id.trim().is_empty() {
        return Err(ForgeError::InvalidState("template publish: empty template id".into()));
    }
    Ok(())
}

/// 内存实现。
#[derive(Default)]
pub struct InMemoryTemplateRegistry {
    inner: Arc<RwLock<HashMap<(String, String), TemplateRecord>>>,
}

impl InMemoryTemplateRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl TemplateRegistry for InMemoryTemplateRegistry {
    async fn publish(&self, rec: TemplateRecord) -> ForgeResult<()> {
        validate_rec(&rec)?;
        let key = (rec.template.id.clone(), rec.version.clone());
        let mut g = self.inner.write().await;
        if g.contains_key(&key) {
            return Err(ForgeError::InvalidState(format!(
                "template {}@{} already published",
                key.0, key.1
            )));
        }
        g.insert(key, rec);
        Ok(())
    }

    async fn get(&self, id: &str, version: &str) -> ForgeResult<TemplateRecord> {
        self.inner
            .read()
            .await
            .get(&(id.to_string(), version.to_string()))
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("template {id}@{version}")))
    }

    async fn versions(&self, id: &str) -> ForgeResult<Vec<TemplateRecord>> {
        let mut v: Vec<_> = self
            .inner
            .read()
            .await
            .iter()
            .filter(|((tid, _), _)| tid == id)
            .map(|(_, r)| r.clone())
            .collect();
        v.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(v)
    }

    async fn list(&self) -> ForgeResult<Vec<TemplateRecord>> {
        let mut v: Vec<_> = self.inner.read().await.values().cloned().collect();
        v.sort_by(|a, b| (&a.template.id, &a.version).cmp(&(&b.template.id, &b.version)));
        Ok(v)
    }
}
