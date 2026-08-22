//! forge-verify：验证抽象与内置验证器。
//!
//! 使 `CheckSpec` 可被机器执行。落实 AP-009：验证是完成条件。

pub mod command;
pub mod file;

use async_trait::async_trait;
use forge_core::{ForgeResult, TaskId};
use forge_task::{AcceptanceCriterion, CheckSpec};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 验证请求。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationRequest {
    /// 任务 ID。
    pub task_id: TaskId,
    /// 验收标准。
    pub criterion: AcceptanceCriterion,
    /// 相对路径的解析基准。
    pub workdir: PathBuf,
}

/// 验证裁决。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// 通过。
    Pass,
    /// 失败。
    Fail,
    /// 无法判定。
    Inconclusive,
}

/// 验证结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationOutcome {
    /// 关联的验收标准 ID。
    pub criterion_id: String,
    /// 裁决。
    pub verdict: Verdict,
    /// 原因（必填：通过/失败/无法判定的原因）。
    pub reason: String,
}

/// 验证器 trait。
#[async_trait]
pub trait Verifier: Send + Sync {
    /// 执行验证。
    async fn verify(&self, req: &VerificationRequest) -> ForgeResult<VerificationOutcome>;
}

/// 根据检查规格选择验证器。
pub fn select_verifier(check: &CheckSpec) -> &'static str {
    match check {
        CheckSpec::Command(_) => "CommandVerifier",
        CheckSpec::FileExists(_) | CheckSpec::FileContains { .. } => "FileVerifier",
    }
}

pub use command::CommandVerifier;
pub use file::FileVerifier;
