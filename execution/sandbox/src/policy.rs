//! 权限策略实现。

use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, PermissionPolicy, PolicyContext};

// 重新导出 PolicyContext 供测试使用
pub use forge_exec::PolicyContext as _PolicyContext;

/// 拒绝所有请求的策略。
pub struct DenyAllPolicy;

impl PermissionPolicy for DenyAllPolicy {
    fn check(&self, _level: PermissionLevel, ctx: &PolicyContext) -> ForgeResult<()> {
        Err(ForgeError::PermissionDenied(format!(
            "denied by DenyAllPolicy: tool={}, level={:?}",
            ctx.tool_name, _level
        )))
    }
}

/// 白名单策略：只允许指定级别。
///
/// **默认策略 = `AllowListPolicy { allowed: vec![ReadOnly] }`**。
pub struct AllowListPolicy {
    /// 允许的权限级别列表。
    pub allowed: Vec<PermissionLevel>,
}

impl Default for AllowListPolicy {
    /// 默认策略：只允许 ReadOnly。
    fn default() -> Self {
        Self {
            allowed: vec![PermissionLevel::ReadOnly],
        }
    }
}

impl PermissionPolicy for AllowListPolicy {
    fn check(&self, level: PermissionLevel, ctx: &PolicyContext) -> ForgeResult<()> {
        if self.allowed.contains(&level) {
            Ok(())
        } else {
            Err(ForgeError::PermissionDenied(format!(
                "tool {} requires {:?}, allowed: {:?}",
                ctx.tool_name, level, self.allowed
            )))
        }
    }
}

/// 默认策略的别名。
pub type DefaultPolicy = AllowListPolicy;

/// 策略链：多条策略按序检查，任一拒绝即拒绝。
///
/// 不提供"跳过检查"的构造路径。
pub struct PolicyChain {
    policies: Vec<Box<dyn PermissionPolicy>>,
}

impl PolicyChain {
    /// 创建空策略链。
    pub fn new() -> Self {
        Self { policies: Vec::new() }
    }

    /// 添加策略。
    pub fn with(mut self, policy: Box<dyn PermissionPolicy>) -> Self {
        self.policies.push(policy);
        self
    }
}

impl Default for PolicyChain {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionPolicy for PolicyChain {
    fn check(&self, level: PermissionLevel, ctx: &PolicyContext) -> ForgeResult<()> {
        for policy in &self.policies {
            policy.check(level, ctx)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_agent::AgentRole;
    use forge_core::SessionId;

    fn make_ctx(tool_name: &str) -> PolicyContext {
        PolicyContext {
            session_id: SessionId::new_session_id(),
            tool_name: tool_name.into(),
            requester_role: AgentRole::Builder,
        }
    }

    #[test]
    fn test_default_policy_readonly_passes() {
        let policy = AllowListPolicy::default();
        let ctx = make_ctx("read_tool");
        assert!(policy.check(PermissionLevel::ReadOnly, &ctx).is_ok());
    }

    #[test]
    fn test_default_policy_write_denied() {
        let policy = AllowListPolicy::default();
        let ctx = make_ctx("write_tool");
        assert!(policy.check(PermissionLevel::WorkspaceWrite, &ctx).is_err());
    }

    #[test]
    fn test_default_policy_external_denied() {
        let policy = AllowListPolicy::default();
        let ctx = make_ctx("ext_tool");
        assert!(policy.check(PermissionLevel::External, &ctx).is_err());
    }

    #[test]
    fn test_default_policy_irreversible_denied() {
        let policy = AllowListPolicy::default();
        let ctx = make_ctx("irr_tool");
        assert!(policy.check(PermissionLevel::Irreversible, &ctx).is_err());
    }

    #[test]
    fn test_deny_all_rejects_everything() {
        let policy = DenyAllPolicy;
        let ctx = make_ctx("any_tool");
        assert!(policy.check(PermissionLevel::ReadOnly, &ctx).is_err());
    }

    #[test]
    fn test_policy_chain_deny_all_blocks() {
        let chain = PolicyChain::new()
            .with(Box::new(AllowListPolicy::default()))
            .with(Box::new(DenyAllPolicy));

        let ctx = make_ctx("tool");
        assert!(chain.check(PermissionLevel::ReadOnly, &ctx).is_err());
    }

    #[test]
    fn test_policy_chain_all_pass() {
        let chain = PolicyChain::new()
            .with(Box::new(AllowListPolicy {
                allowed: vec![PermissionLevel::ReadOnly, PermissionLevel::WorkspaceWrite],
            }))
            .with(Box::new(AllowListPolicy {
                allowed: vec![PermissionLevel::ReadOnly, PermissionLevel::WorkspaceWrite],
            }));

        let ctx = make_ctx("tool");
        assert!(chain.check(PermissionLevel::WorkspaceWrite, &ctx).is_ok());
    }

    #[test]
    fn test_denial_message_contains_tool_name() {
        let policy = AllowListPolicy::default();
        let ctx = make_ctx("my_special_tool");
        let err = policy.check(PermissionLevel::External, &ctx).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("my_special_tool"), "error should contain tool name: {}", msg);
    }

    #[test]
    fn test_allow_list_custom() {
        let policy = AllowListPolicy {
            allowed: vec![
                PermissionLevel::ReadOnly,
                PermissionLevel::WorkspaceWrite,
                PermissionLevel::External,
            ],
        };
        let ctx = make_ctx("tool");
        assert!(policy.check(PermissionLevel::ReadOnly, &ctx).is_ok());
        assert!(policy.check(PermissionLevel::WorkspaceWrite, &ctx).is_ok());
        assert!(policy.check(PermissionLevel::External, &ctx).is_ok());
        assert!(policy.check(PermissionLevel::Irreversible, &ctx).is_err());
    }
}
