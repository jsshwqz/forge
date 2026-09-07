//! V7 ORCH-101b：沙箱化命令验收（build_v70a.md 契约，D10 红线解除面）。
//!
//! R1 策略链冻结：AllowListPolicy [WorkspaceWrite, External]——写盘与命令放行，
//! Irreversible 拒绝且**不可配置放宽**；R2 拒绝留证：verdict=Fail 且 reason 带
//! `sandbox_policy` 前缀（编排器把 reason 写入 evidence content——produced_by 由
//! 编排器冻结为 verifier_name，审计标记落在内容前缀，规格适配记 R1-071）。

use forge_core::{ForgeResult, SessionId};
use forge_agent::AgentRole;
use forge_exec::{PermissionLevel, PolicyContext};
use forge_task::CheckSpec;
use forge_verify::{Verifier, VerificationOutcome, Verdict, VerificationRequest};
use std::sync::Arc;

/// 拒绝审计标记（冻结：进 evidence content 可检索）。
pub const SANDBOX_DENY_MARKER: &str = "sandbox_policy";

/// 不可逆命令模式（冻结清单，不可配置放宽——D10 第 3 条）。
const IRREVERSIBLE_PATTERNS: &[&str] = &[
    "format ", "format/", "mkfs", "diskpart", "rd /s", "rm -rf /", "dd if=", "del /f /s /q",
];

/// 命令风险分级：命中不可逆模式 → Irreversible；其余 External。
pub fn command_level(cmd: &str) -> PermissionLevel {
    let lower = cmd.to_ascii_lowercase();
    if IRREVERSIBLE_PATTERNS.iter().any(|p| lower.contains(p)) {
        PermissionLevel::Irreversible
    } else {
        PermissionLevel::External
    }
}

/// 生产沙箱策略链（R1 冻结：[WorkspaceWrite, External]，不含 Irreversible）。
pub fn sandbox_policy() -> Arc<dyn forge_exec::PermissionPolicy> {
    Arc::new(forge_sandbox::PolicyChain::new().with(Box::new(forge_sandbox::AllowListPolicy {
        allowed: vec![PermissionLevel::WorkspaceWrite, PermissionLevel::External],
    })))
}

/// 沙箱化命令验收器：包装既有 CommandVerifier，执行前过策略链（R2 拒绝留证）。
pub struct SandboxCommandVerifier {
    inner: Arc<dyn Verifier>,
    policy: Arc<dyn forge_exec::PermissionPolicy>,
}

impl SandboxCommandVerifier {
    pub fn new(inner: Arc<dyn Verifier>, policy: Arc<dyn forge_exec::PermissionPolicy>) -> Self {
        Self { inner, policy }
    }
}

#[async_trait::async_trait]
impl Verifier for SandboxCommandVerifier {
    async fn verify(&self, req: &VerificationRequest) -> ForgeResult<VerificationOutcome> {
        let level = match &req.criterion.check {
            CheckSpec::Command(cmd) => command_level(cmd),
            _ => PermissionLevel::External,
        };
        let ctx = PolicyContext {
            session_id: SessionId::new_session_id(),
            tool_name: format!("acceptance:{}", req.criterion.id),
            requester_role: AgentRole::Builder,
        };
        if let Err(e) = self.policy.check(level, &ctx) {
            return Ok(VerificationOutcome {
                criterion_id: req.criterion.id.clone(),
                verdict: Verdict::Fail,
                reason: format!("{SANDBOX_DENY_MARKER}: {e}"),
            });
        }
        self.inner.verify(req).await
    }
}

/// 验收器装配（R4 冻结：仅 MultiStep 走沙箱包装；基线路径保持原 verifier 零回归）。
/// 返回 (verifier, uses_sandbox)。
pub fn select_command_verifier(
    plan_mode: crate::PlanMode,
    inner: Arc<dyn Verifier>,
) -> (Arc<dyn Verifier>, bool) {
    match plan_mode {
        crate::PlanMode::MultiStep => (
            Arc::new(SandboxCommandVerifier::new(inner, sandbox_policy())),
            true,
        ),
        _ => (inner, false),
    }
}
