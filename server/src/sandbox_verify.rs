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
use std::path::Path;
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

// ==================== V8 SANDBOX-002：容器级验收隔离（默认关） ====================

/// 容器运行参数冻结形态（不含 runtime 前缀，`docker run` / `podman run` 直接拼接）。
pub fn container_run_args(image: &str, workdir_host: &Path, cmd: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        "--network=none".to_string(),
        "--memory=512m".to_string(),
        "--pids-limit=100".to_string(),
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "-v".to_string(),
        format!("{}:/work:rw", workdir_host.display()),
        "-w".to_string(),
        "/work".to_string(),
        "--entrypoint".to_string(),
        "/bin/sh".to_string(),
        image.to_string(),
        "-lc".to_string(),
        cmd.to_string(),
    ]
}

/// 容器隔离是否启用：`FORGE_SANDBOX_CONTAINER=1` 显式开启，缺省关闭（R2 零变化）。
pub fn container_enabled() -> bool {
    std::env::var("FORGE_SANDBOX_CONTAINER").ok().as_deref() == Some("1")
}

/// 容器镜像（默认 docker.m.daocloud.io/library/node:20-alpine）。
pub fn container_image() -> String {
    std::env::var("FORGE_SANDBOX_IMAGE")
        .unwrap_or_else(|_| "docker.m.daocloud.io/library/node:20-alpine".to_string())
}

/// 容器运行时（docker 或 podman，默认 docker；Windows/Podman 挂载语义差异留待演练）。
pub fn container_runtime() -> String {
    std::env::var("FORGE_SANDBOX_RUNTIME").unwrap_or_else(|_| "docker".to_string())
}

/// 容器验收器：黑名单（策略链）为第一道闸，容器执行为第二道（纵深防御）。
/// 仅 `container_enabled()` 时由 [`select_command_verifier`] 选用；缺省关闭走本地。    
pub struct ContainerCommandVerifier {
    inner: Arc<dyn Verifier>,
    policy: Arc<dyn forge_exec::PermissionPolicy>,
}

impl ContainerCommandVerifier {
    pub fn new(inner: Arc<dyn Verifier>, policy: Arc<dyn forge_exec::PermissionPolicy>) -> Self {
        Self { inner, policy }
    }
}

#[async_trait::async_trait]
impl Verifier for ContainerCommandVerifier {
    async fn verify(&self, req: &VerificationRequest) -> ForgeResult<VerificationOutcome> {
        let cmd_str = match &req.criterion.check {
            CheckSpec::Command(c) => c.clone(),
            // 非命令检查（FileContains/FileExists）委托内层，不进容器。
            _ => return self.inner.verify(req).await,
        };

        // R1：策略链（黑名单）仍为第一道闸。
        let level = command_level(&cmd_str);
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

        // R3：容器执行失败/非零退出 → Fail（reason 带 container: 前缀）。
        let runtime = container_runtime();
        let image = container_image();
        let args = container_run_args(&image, &req.workdir, &cmd_str);
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(&runtime).args(&args).output()
        })
        .await
        .map_err(|e| forge_core::ForgeError::InvalidState(format!("container join: {e}")))?;

        match output {
            Ok(out) if out.status.success() => Ok(VerificationOutcome {
                criterion_id: req.criterion.id.clone(),
                verdict: Verdict::Pass,
                reason: format!(
                    "container: command succeeded: {}",
                    String::from_utf8_lossy(&out.stdout).trim()
                ),
            }),
            Ok(out) => Ok(VerificationOutcome {
                criterion_id: req.criterion.id.clone(),
                verdict: Verdict::Fail,
                reason: format!(
                    "container: exit code {:?}: {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            }),
            Err(e) => Ok(VerificationOutcome {
                criterion_id: req.criterion.id.clone(),
                verdict: Verdict::Fail,
                reason: format!("container: failed to execute: {e}"),
            }),
        }
    }
}

/// 验收器装配（R4 冻结：仅 MultiStep 走沙箱包装；基线路径保持原 verifier 零回归）。
/// 返回 (verifier, uses_sandbox)。
pub fn select_command_verifier(
    plan_mode: crate::PlanMode,
    inner: Arc<dyn Verifier>,
) -> (Arc<dyn Verifier>, bool) {
    match plan_mode {
        crate::PlanMode::MultiStep => {
            // V8 SANDBOX-002：FORGE_SANDBOX_CONTAINER=1 时容器为第二道闸；缺省零变化。
            if container_enabled() {
                (
                    Arc::new(ContainerCommandVerifier::new(inner, sandbox_policy())),
                    true,
                )
            } else {
                (
                    Arc::new(SandboxCommandVerifier::new(inner, sandbox_policy())),
                    true,
                )
            }
        }
        _ => (inner, false),
    }
}
