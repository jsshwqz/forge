//! 门禁裁决实现。

use forge_core::TaskId;
use forge_verify::VerificationOutcome;
use forge_verify::Verdict;
use serde::{Deserialize, Serialize};

/// 门禁策略。第一阶段只有 AllPass。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatePolicy {
    /// 全部通过才放行。
    AllPass,
}

/// 门禁规格。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateSpec {
    /// 任务 ID。
    pub task_id: TaskId,
    /// 要求的验收标准 ID 列表。
    pub required_criterion_ids: Vec<String>,
    /// 门禁策略。
    pub policy: GatePolicy,
}

/// 门禁决策结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateDecision {
    /// 是否通过。
    pub passed: bool,
    /// 实际评估到的标准数。
    pub evaluated: usize,
    /// 要求了但没有验证结果的标准。
    pub missing: Vec<String>,
    /// verdict != Pass 的标准。
    pub failed: Vec<String>,
}

/// 门禁裁决器。
///
/// **调用方义务：只有 `passed == true` 才允许把 Task 置为 Completed**
/// （与 AF-TASK-001 的空验收禁令互为双保险）。
pub struct Gate;

impl Gate {
    /// 评估门禁。纯函数，可离线复算。
    ///
    /// - outcomes 不足/有缺失/有 Fail → passed=false
    /// - 缺失（要求了但没结果）与失败分开报告
    /// - 空要求列表 → passed=false（禁止空门禁放行）
    pub fn evaluate(spec: &GateSpec, outcomes: &[VerificationOutcome]) -> GateDecision {
        // 空要求列表 → 禁止放行
        if spec.required_criterion_ids.is_empty() {
            return GateDecision {
                passed: false,
                evaluated: 0,
                missing: vec![],
                failed: vec![],
            };
        }

        let outcome_map: std::collections::HashMap<&str, &VerificationOutcome> = outcomes
            .iter()
            .map(|o| (o.criterion_id.as_str(), o))
            .collect();

        let mut missing = Vec::new();
        let mut failed = Vec::new();
        let mut evaluated = 0;

        for req_id in &spec.required_criterion_ids {
            match outcome_map.get(req_id.as_str()) {
                Some(outcome) => {
                    evaluated += 1;
                    if outcome.verdict != Verdict::Pass {
                        failed.push(req_id.clone());
                    }
                }
                None => {
                    missing.push(req_id.clone());
                }
            }
        }

        let passed = missing.is_empty() && failed.is_empty();

        GateDecision {
            passed,
            evaluated,
            missing,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(ids: Vec<&str>) -> GateSpec {
        GateSpec {
            task_id: TaskId::new_task_id(),
            required_criterion_ids: ids.into_iter().map(String::from).collect(),
            policy: GatePolicy::AllPass,
        }
    }

    fn make_outcome(id: &str, verdict: Verdict) -> VerificationOutcome {
        VerificationOutcome {
            criterion_id: id.into(),
            verdict,
            reason: "test".into(),
        }
    }

    #[test]
    fn test_all_pass() {
        let spec = make_spec(vec!["AC-1", "AC-2", "AC-3"]);
        let outcomes = vec![
            make_outcome("AC-1", Verdict::Pass),
            make_outcome("AC-2", Verdict::Pass),
            make_outcome("AC-3", Verdict::Pass),
        ];
        let decision = Gate::evaluate(&spec, &outcomes);
        assert!(decision.passed);
        assert_eq!(decision.evaluated, 3);
        assert!(decision.missing.is_empty());
        assert!(decision.failed.is_empty());
    }

    #[test]
    fn test_one_fail() {
        let spec = make_spec(vec!["AC-1", "AC-2"]);
        let outcomes = vec![
            make_outcome("AC-1", Verdict::Pass),
            make_outcome("AC-2", Verdict::Fail),
        ];
        let decision = Gate::evaluate(&spec, &outcomes);
        assert!(!decision.passed);
        assert!(decision.failed.contains(&"AC-2".to_string()));
    }

    #[test]
    fn test_missing_results() {
        let spec = make_spec(vec!["AC-1", "AC-2", "AC-3"]);
        let outcomes = vec![
            make_outcome("AC-1", Verdict::Pass),
            make_outcome("AC-2", Verdict::Pass),
            // AC-3 缺失
        ];
        let decision = Gate::evaluate(&spec, &outcomes);
        assert!(!decision.passed);
        assert!(decision.missing.contains(&"AC-3".to_string()));
        assert!(decision.failed.is_empty());
    }

    #[test]
    fn test_empty_required_rejected() {
        let spec = make_spec(vec![]);
        let outcomes = vec![make_outcome("AC-1", Verdict::Pass)];
        let decision = Gate::evaluate(&spec, &outcomes);
        assert!(!decision.passed, "empty gate must not pass");
    }

    #[test]
    fn test_missing_and_failed_separate() {
        let spec = make_spec(vec!["AC-1", "AC-2", "AC-3"]);
        let outcomes = vec![
            make_outcome("AC-1", Verdict::Pass),
            make_outcome("AC-2", Verdict::Fail),
            // AC-3 缺失
        ];
        let decision = Gate::evaluate(&spec, &outcomes);
        assert!(!decision.passed);
        assert!(decision.failed.contains(&"AC-2".to_string()));
        assert!(decision.missing.contains(&"AC-3".to_string()));
    }
}
