//! Product Manifest 模型与校验。

use forge_agent::AgentRole;
use forge_cap::CapabilityRegistry;
use forge_core::{ForgeError, ForgeResult, ProductId};
use serde::{Deserialize, Serialize};

/// 能力引用。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityRef {
    /// 能力名称。
    pub capability_name: String,
    /// 版本。
    pub version: String,
    /// 是否必填。
    pub required: bool,
}

/// 产品清单。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductManifest {
    /// 产品 ID。
    pub id: ProductId,
    /// 名称。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 描述。
    pub description: String,
    /// 能力引用列表。
    pub capabilities: Vec<CapabilityRef>,
    /// 入口 Agent 角色（第一阶段冻结为 Orchestrator）。
    pub entry_agent_role: AgentRole,
}

impl ProductManifest {
    /// 校验入口角色。
    pub fn validate(&self) -> ForgeResult<()> {
        if self.entry_agent_role != AgentRole::Orchestrator {
            return Err(ForgeError::InvalidState(format!(
                "entry_agent_role must be Orchestrator in first phase, got {:?}",
                self.entry_agent_role
            )));
        }
        Ok(())
    }
}

/// 对照注册表校验：返回缺失的必填能力列表（空 = 全部满足）。
///
/// 只查 `required=true` 的引用；`required=false` 缺失不报告。
/// 校验是只读操作，不修改注册表。
pub async fn check_manifest(
    manifest: &ProductManifest,
    registry: &dyn CapabilityRegistry,
) -> ForgeResult<Vec<CapabilityRef>> {
    let mut missing = Vec::new();

    for cap_ref in &manifest.capabilities {
        if !cap_ref.required {
            continue;
        }
        let found = registry
            .find_by_name(&cap_ref.capability_name)
            .await
            .unwrap_or_default();
        let version_match = found.iter().any(|c| c.version == cap_ref.version);
        if !version_match {
            missing.push(cap_ref.clone());
        }
    }

    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cap::{
        Capability, CapabilityKind, CapabilityStatus, InMemoryCapabilityRegistry,
    };
    use forge_exec::PermissionLevel;

    fn make_cap(name: &str, version: &str) -> Capability {
        Capability {
            id: forge_core::new_capability_id(),
            name: name.into(),
            kind: CapabilityKind::Tool,
            version: version.into(),
            entry: "/path".into(),
            status: CapabilityStatus::Registered,
            permission: PermissionLevel::ReadOnly,
        }
    }

    fn make_manifest(caps: Vec<CapabilityRef>) -> ProductManifest {
        ProductManifest {
            id: ProductId::new_product_id(),
            name: "test-product".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            capabilities: caps,
            entry_agent_role: AgentRole::Orchestrator,
        }
    }

    #[tokio::test]
    async fn test_all_satisfied() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0")).await.unwrap();

        let manifest = make_manifest(vec![CapabilityRef {
            capability_name: "echo".into(),
            version: "1.0.0".into(),
            required: true,
        }]);

        let missing = check_manifest(&manifest, &reg).await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn test_missing_required() {
        let reg = InMemoryCapabilityRegistry::default();

        let manifest = make_manifest(vec![CapabilityRef {
            capability_name: "missing-cap".into(),
            version: "1.0.0".into(),
            required: true,
        }]);

        let missing = check_manifest(&manifest, &reg).await.unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].capability_name, "missing-cap");
    }

    #[tokio::test]
    async fn test_missing_optional_not_reported() {
        let reg = InMemoryCapabilityRegistry::default();

        let manifest = make_manifest(vec![CapabilityRef {
            capability_name: "optional-cap".into(),
            version: "1.0.0".into(),
            required: false,
        }]);

        let missing = check_manifest(&manifest, &reg).await.unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn test_validate_non_orchestrator_rejected() {
        let manifest = ProductManifest {
            id: ProductId::new_product_id(),
            name: "test".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            capabilities: vec![],
            entry_agent_role: AgentRole::Builder,
        };
        assert!(manifest.validate().is_err());
    }
}
