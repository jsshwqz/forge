//! 文件验证器：FileExists / FileContains。

use crate::{Verifier, VerificationOutcome, VerificationRequest, Verdict};
use async_trait::async_trait;
use forge_core::ForgeResult;
use forge_task::CheckSpec;

/// 文件验证器。
pub struct FileVerifier;

#[async_trait]
impl Verifier for FileVerifier {
    async fn verify(&self, req: &VerificationRequest) -> ForgeResult<VerificationOutcome> {
        match &req.criterion.check {
            CheckSpec::FileExists(rel_path) => {
                let full_path = req.workdir.join(rel_path);
                if full_path.exists() {
                    Ok(VerificationOutcome {
                        criterion_id: req.criterion.id.clone(),
                        verdict: Verdict::Pass,
                        reason: format!("file exists: {}", rel_path),
                    })
                } else {
                    Ok(VerificationOutcome {
                        criterion_id: req.criterion.id.clone(),
                        verdict: Verdict::Fail,
                        reason: format!("file not found: {}", rel_path),
                    })
                }
            }
            CheckSpec::FileContains { path: rel_path, needle } => {
                let full_path = req.workdir.join(rel_path);
                match std::fs::read_to_string(&full_path) {
                    Ok(content) => {
                        if content.contains(needle.as_str()) {
                            Ok(VerificationOutcome {
                                criterion_id: req.criterion.id.clone(),
                                verdict: Verdict::Pass,
                                reason: format!("found '{}' in {}", needle, rel_path),
                            })
                        } else {
                            Ok(VerificationOutcome {
                                criterion_id: req.criterion.id.clone(),
                                verdict: Verdict::Fail,
                                reason: format!("'{}' not found in {}", needle, rel_path),
                            })
                        }
                    }
                    Err(e) => {
                        Ok(VerificationOutcome {
                            criterion_id: req.criterion.id.clone(),
                            verdict: Verdict::Fail,
                            reason: format!("cannot read file {}: {}", rel_path, e),
                        })
                    }
                }
            }
            CheckSpec::Command(_) => {
                Ok(VerificationOutcome {
                    criterion_id: req.criterion.id.clone(),
                    verdict: Verdict::Inconclusive,
                    reason: "FileVerifier cannot handle command checks".into(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::TaskId;
    use forge_task::{AcceptanceCriterion, CheckSpec};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_file_exists_pass() {
        let verifier = FileVerifier;
        let req = VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::FileExists("Cargo.toml".into()),
            },
            workdir: PathBuf::from("../.."),
        };
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Pass);
    }

    #[tokio::test]
    async fn test_file_exists_fail() {
        let verifier = FileVerifier;
        let req = VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::FileExists("nonexistent_file.txt".into()),
            },
            workdir: PathBuf::from("."),
        };
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Fail);
    }

    #[tokio::test]
    async fn test_file_contains_pass() {
        let verifier = FileVerifier;
        let req = VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::FileContains {
                    path: "Cargo.toml".into(),
                    needle: "[workspace]".into(),
                },
            },
            workdir: PathBuf::from("../.."),
        };
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Pass);
    }

    #[tokio::test]
    async fn test_file_contains_fail() {
        let verifier = FileVerifier;
        let req = VerificationRequest {
            task_id: TaskId::new_task_id(),
            criterion: AcceptanceCriterion {
                id: "AC-1".into(),
                description: "test".into(),
                check: CheckSpec::FileContains {
                    path: "Cargo.toml".into(),
                    needle: "THIS_STRING_DOES_NOT_EXIST".into(),
                },
            },
            workdir: PathBuf::from("../.."),
        };
        let outcome = verifier.verify(&req).await.unwrap();
        assert_eq!(outcome.verdict, Verdict::Fail);
    }
}
