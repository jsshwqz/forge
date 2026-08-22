//! 权限策略 trait 定义。
//!
//! `PermissionPolicy` trait 定义在此 crate 中以避免循环依赖。
//! 具体实现（DenyAllPolicy, AllowListPolicy, PolicyChain）在 forge-sandbox 中。

use crate::permission_level::PermissionLevel;
use forge_agent::AgentRole;
use forge_core::{ForgeResult, SessionId};
use serde::{Deserialize, Serialize};

/// 策略上下文。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyContext {
    /// 会话 ID。
    pub session_id: SessionId,
    /// 工具名称。
    pub tool_name: String,
    /// 请求者角色。
    pub requester_role: AgentRole,
}

/// 权限策略 trait。
///
/// 允许返回 `Ok(())`；拒绝返回 `ForgeError::PermissionDenied(原因)`。
pub trait PermissionPolicy: Send + Sync {
    /// 检查权限。
    fn check(&self, level: PermissionLevel, ctx: &PolicyContext) -> ForgeResult<()>;
}
