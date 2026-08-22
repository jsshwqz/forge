//! 命令验证器：执行命令，退出码 0 = Pass，非 0 = Fail，无法执行 = Inconclusive。

use crate::{Verifier, VerificationOutcome, VerificationRequest, Verdict};
use async_trait::async_trait;
use forge_core::ForgeResult;
use forge_task::CheckSpec;
use std::process::Command;

/// 命令验证器。
pub struct CommandVerifier;

#[async_trait]
impl Verifier for CommandVerifier {
    async fn verify(&self, req: &VerificationRequest) -> ForgeResult<VerificationOutcome> {
        let cmd = match &req.criterion.check {
            CheckSpec::Command(c) => c.clone(),
            _ => {
                return Ok(VerificationOutcome {
                    criterion_id: req.criterion.id.clone(),
                    verdict: Verdict::Inconclusive,
                    reason: format!(
                        "CommandVerifier cannot handle non-command check: {:?}",
                        req.criterion.check
                    ),
                });
            }
        };

        // 跨平台：Windows 用 cmd /C，其他用 sh -c
        let (program, args) = if cfg!(target_os = "windows") {
            ("cmd", vec!["/C".to_string(), cmd])
        } else {
            ("sh", vec!["-c".to_string(), cmd])
        };

        let output = Command::new(program)
            .args(&args)
            .current_dir(&req.workdir)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let combined = format!("{}{}", stdout, stderr);
                let truncated = if combined.len() > 4096 {
                    &combined[..4096]
                } else {
                    &combined
                };

                if out.status.success() {
                    Ok(VerificationOutcome {
                        criterion_id: req.criterion.id.clone(),
                        verdict: Verdict::Pass,
                        reason: format!("command succeeded: {}", truncated.trim()),
                    })
                } else {
                    Ok(VerificationOutcome {
                        criterion_id: req.criterion.id.clone(),
                        verdict: Verdict::Fail,
                        reason: format!(
                            "command exited with code {:?}: {}",
                            out.status.code(),
                            truncated.trim()
                        ),
                    })
                }
            }
            Err(e) => Ok(VerificationOutcome {
                criterion_id: req.criterion.id.clone(),
                verdict: Verdict::Inconclusive,
                reason: format!("failed to execute command: {}", e),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VerificationRequest, Verdict};
    use forge_core::TaskId;
    use forge_task::{AcceptanceCriterion, CheckSpec};
    use std::path::PathBuf;

    fn make_req(cmd: &str) -> VerificationRequest {
        VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::Command(cmd.into()),
            },
            workdir: PathBuf::from("."),
        }
    }

    #[tokio::test]
    async fn test_command_pass() {
        let verifier = CommandVerifier;
        let cmd = if cfg!(target_os = "windows") {
            "cmd /C exit 0"
        } else {
            "true"
        };
        let req = make_req(cmd);
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Pass);
    }

    #[tokio::test]
    async fn test_command_fail() {
        let verifier = CommandVerifier;
        let cmd = if cfg!(target_os = "windows") {
            "cmd /C exit 1"
        } else {
            "false"
        };
        let req = make_req(cmd);
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Fail);
        assert!(!outcome.reason.is_empty());
    }

    #[tokio::test]
    async fn test_non_command_returns_inconclusive() {
        let verifier = CommandVerifier;
        let req = VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::FileExists("test.txt".into()),
            },
            workdir: PathBuf::from("."),
        };
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Inconclusive);
    }
}
