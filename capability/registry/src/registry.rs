//! 能力注册表实现。

use async_trait::async_trait;
use forge_core::{CapabilityId, ForgeError, ForgeResult};
use forge_exec::PermissionLevel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 能力类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityKind {
    Skill,
    Tool,
    McpServer,
    Api,
}

/// 能力状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityStatus {
    Registered,
    Active,
    Deprecated,
}

/// 能力对象。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capability {
    /// 能力 ID。
    pub id: CapabilityId,
    /// 名称。
    pub name: String,
    /// 类型。
    pub kind: CapabilityKind,
    /// 版本（semver 字符串）。
    pub version: String,
    /// 入口描述（路径/命令/URL）。
    pub entry: String,
    /// 状态。
    pub status: CapabilityStatus,
    /// 所需权限级别。
    pub permission: PermissionLevel,
}

/// 能力注册表 trait。
#[async_trait]
pub trait CapabilityRegistry: Send + Sync {
    /// 注册能力。同 `name + version` 重复注册 → `InvalidState`。
    async fn register(&self, cap: Capability) -> ForgeResult<CapabilityId>;

    /// 获取能力。
    async fn get(&self, id: &CapabilityId) -> ForgeResult<Capability>;

    /// 按名称查找。
    async fn find_by_name(&self, name: &str) -> ForgeResult<Vec<Capability>>;

    /// 按类型列举。
    async fn list_by_kind(&self, kind: CapabilityKind) -> ForgeResult<Vec<Capability>>;

    /// 弃用能力。
    async fn deprecate(&self, id: &CapabilityId) -> ForgeResult<()>;
}

/// 内存能力注册表。
#[derive(Default)]
pub struct InMemoryCapabilityRegistry {
    caps: Arc<RwLock<HashMap<CapabilityId, Capability>>>,
}

#[async_trait]
impl CapabilityRegistry for InMemoryCapabilityRegistry {
    async fn register(&self, cap: Capability) -> ForgeResult<CapabilityId> {
        let mut guard = self.caps.write().await;
        // 检查同 name + version
        for existing in guard.values() {
            if existing.name == cap.name && existing.version == cap.version {
                return Err(ForgeError::InvalidState(format!(
                    "capability already registered: {} v{}",
                    cap.name, cap.version
                )));
            }
        }
        let id = cap.id.clone();
        guard.insert(id.clone(), cap);
        Ok(id)
    }

    async fn get(&self, id: &CapabilityId) -> ForgeResult<Capability> {
        self.caps
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("capability: {}", id)))
    }

    async fn find_by_name(&self, name: &str) -> ForgeResult<Vec<Capability>> {
        Ok(self
            .caps
            .read()
            .await
            .values()
            .filter(|c| c.name == name)
            .cloned()
            .collect())
    }

    async fn list_by_kind(&self, kind: CapabilityKind) -> ForgeResult<Vec<Capability>> {
        Ok(self
            .caps
            .read()
            .await
            .values()
            .filter(|c| c.kind == kind)
            .cloned()
            .collect())
    }

    async fn deprecate(&self, id: &CapabilityId) -> ForgeResult<()> {
        let mut guard = self.caps.write().await;
        let cap = guard
            .get_mut(id)
            .ok_or_else(|| ForgeError::NotFound(format!("capability: {}", id)))?;
        cap.status = CapabilityStatus::Deprecated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cap(name: &str, version: &str, kind: CapabilityKind) -> Capability {
        Capability {
            id: CapabilityId::new_capability_id(),
            name: name.into(),
            kind,
            version: version.into(),
            entry: "/path/to/entry".into(),
            status: CapabilityStatus::Registered,
            permission: PermissionLevel::ReadOnly,
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let reg = InMemoryCapabilityRegistry::default();
        let cap = make_cap("echo", "1.0.0", CapabilityKind::Tool);
        let id = reg.register(cap.clone()).await.unwrap();
        let got = reg.get(&id).await.unwrap();
        assert_eq!(got.name, "echo");
    }

    #[tokio::test]
    async fn test_duplicate_name_version_rejected() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0", CapabilityKind::Tool))
            .await
            .unwrap();
        let result = reg.register(make_cap("echo", "1.0.0", CapabilityKind::Tool)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_by_name() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0", CapabilityKind::Tool)).await.unwrap();
        reg.register(make_cap("echo", "2.0.0", CapabilityKind::Tool)).await.unwrap();
        reg.register(make_cap("other", "1.0.0", CapabilityKind::Skill)).await.unwrap();

        let results = reg.find_by_name("echo").await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_list_by_kind() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("a", "1.0.0", CapabilityKind::Tool)).await.unwrap();
        reg.register(make_cap("b", "1.0.0", CapabilityKind::Skill)).await.unwrap();
        reg.register(make_cap("c", "1.0.0", CapabilityKind::Tool)).await.unwrap();

        let tools = reg.list_by_kind(CapabilityKind::Tool).await.unwrap();
        assert_eq!(tools.len(), 2);
        let skills = reg.list_by_kind(CapabilityKind::Skill).await.unwrap();
        assert_eq!(skills.len(), 1);
    }

    #[tokio::test]
    async fn test_deprecate() {
        let reg = InMemoryCapabilityRegistry::default();
        let id = reg.register(make_cap("echo", "1.0.0", CapabilityKind::Tool)).await.unwrap();
        reg.deprecate(&id).await.unwrap();
        let got = reg.get(&id).await.unwrap();
        assert_eq!(got.status, CapabilityStatus::Deprecated);
    }

    #[tokio::test]
    async fn test_not_found() {
        let reg = InMemoryCapabilityRegistry::default();
        assert!(reg.get(&CapabilityId::new_capability_id()).await.is_err());
    }
}
