//! 角色档案（AGENT-P-001）：四角色的可序列化配置。
//!
//! 档案可经 forge-cap 能力注册表分发：`to_capability()` 生成 Skill 型条目
//! （entry = 档案 JSON），对端 `from_capability()` 还原。

use forge_cap::{Capability, CapabilityKind, CapabilityStatus, CapabilityRegistry};
use forge_core::{ForgeError, ForgeResult};
use forge_exec::PermissionLevel;
use serde::{Deserialize, Serialize};

/// 模型档位（成本分层）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier {
    /// 高档：战略/审查等高价值推理。
    High,
    /// 低档：执行/机械性调用。
    Low,
}

/// 流水线角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Architect,
    Builder,
    Tester,
    Reviewer,
}

impl Role {
    /// 角色名的稳定字符串形式（用于 capability 命名与日志）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Architect => "architect",
            Role::Builder => "builder",
            Role::Tester => "tester",
            Role::Reviewer => "reviewer",
        }
    }
}

/// 角色档案：{ role, model_tier, system_prompt_id, permission_profile }。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleProfile {
    pub role: Role,
    pub model_tier: ModelTier,
    /// 系统提示词模板 ID（提示词库键，本阶段仅透传记录）。
    pub system_prompt_id: String,
    /// 权限档名（映射到 PermissionLevel 的策略键）。
    pub permission_profile: String,
}

impl RoleProfile {
    /// 序列化为 JSON（分发载体）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// 从 JSON 还原。
    pub fn from_json(s: &str) -> ForgeResult<Self> {
        serde_json::from_str(s)
            .map_err(|e| ForgeError::InvalidState(format!("role profile json: {e}")))
    }

    /// 打包为能力注册表条目（Skill 型，entry=档案 JSON），供 registry 分发。
    pub fn to_capability(&self) -> Capability {
        Capability {
            id: forge_core::new_capability_id(),
            name: format!("role/{}", self.role.as_str()),
            kind: CapabilityKind::Skill,
            version: "0.1.0".into(),
            entry: self.to_json(),
            status: CapabilityStatus::Active,
            permission: PermissionLevel::WorkspaceWrite,
        }
    }
}

/// 权限档名 → PermissionLevel 的默认映射（permission_profile 协议）。
pub fn permission_level_of(profile: &str) -> PermissionLevel {
    match profile {
        "readonly" => PermissionLevel::ReadOnly,
        "workspace" | "" => PermissionLevel::WorkspaceWrite,
        "external" => PermissionLevel::External,
        "irreversible" => PermissionLevel::Irreversible,
        _ => PermissionLevel::WorkspaceWrite,
    }
}

/// 默认四角色档案：战略/审查用高档，执行/验收用低档（Tester 为确定性 Verifier，
/// 档位字段保留仅为档案完整性）。
pub fn default_profiles() -> Vec<RoleProfile> {
    vec![
        RoleProfile {
            role: Role::Architect,
            model_tier: ModelTier::High,
            system_prompt_id: "pipeline.architect".into(),
            permission_profile: "workspace".into(),
        },
        RoleProfile {
            role: Role::Builder,
            model_tier: ModelTier::Low,
            system_prompt_id: "pipeline.builder".into(),
            permission_profile: "workspace".into(),
        },
        RoleProfile {
            role: Role::Tester,
            model_tier: ModelTier::Low,
            system_prompt_id: "pipeline.tester".into(),
            permission_profile: "readonly".into(),
        },
        RoleProfile {
            role: Role::Reviewer,
            model_tier: ModelTier::High,
            system_prompt_id: "pipeline.reviewer".into(),
            permission_profile: "readonly".into(),
        },
    ]
}

/// 把全部角色档案注册进能力注册表并回读校验（分发闭环）。
pub async fn distribute_profiles(
    registry: &dyn CapabilityRegistry,
    profiles: &[RoleProfile],
) -> ForgeResult<()> {
    for p in profiles {
        let cap = p.to_capability();
        let id = registry.register(cap).await?;
        // 回读验证内容一致（防 entry 截断/编码问题）
        let got = registry.get(&id).await?;
        if RoleProfile::from_json(&got.entry)? != *p {
            return Err(ForgeError::InvalidState(format!(
                "profile roundtrip mismatch for {}",
                p.role.as_str()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cap::InMemoryCapabilityRegistry;

    #[test]
    fn serde_roundtrip() {
        let p = default_profiles().remove(0);
        let s = p.to_json();
        assert_eq!(RoleProfile::from_json(&s).unwrap(), p);
    }

    #[test]
    fn bad_json_rejected() {
        assert!(RoleProfile::from_json("not json").is_err());
    }

    #[test]
    fn default_profiles_cover_four_roles() {
        let ps = default_profiles();
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0].model_tier, ModelTier::High);
        assert_eq!(ps[1].model_tier, ModelTier::Low);
        assert_eq!(ps[3].model_tier, ModelTier::High);
    }

    #[tokio::test]
    async fn distributes_via_capability_registry() {
        let reg = InMemoryCapabilityRegistry::default();
        let profiles = default_profiles();
        distribute_profiles(&reg, &profiles).await.unwrap();

        let found = reg.find_by_name("role/reviewer").await.unwrap();
        assert_eq!(found.len(), 1);
        let back = RoleProfile::from_json(&found[0].entry).unwrap();
        assert_eq!(back.role, Role::Reviewer);
        assert_eq!(back.model_tier, ModelTier::High);
    }

    #[test]
    fn permission_mapping() {
        assert!(matches!(permission_level_of("readonly"), PermissionLevel::ReadOnly));
        assert!(matches!(permission_level_of("external"), PermissionLevel::External));
        assert!(matches!(permission_level_of(""), PermissionLevel::WorkspaceWrite));
    }
}
