//! Plan 结构校验（IMPROVE-9 路径 B）。
//!
//! 纯函数，零 IO：检查 steps 非空、id 唯一、规模上限，复用 `build_dag` 做环/悬空检测。

use forge_core::{ForgeError, ForgeResult};
use forge_planner::Plan;
use std::collections::HashSet;

/// 默认步骤数上限（env `FORGE_PLAN_MAX_STEPS` 可调）。
pub const FORGE_PLAN_MAX_STEPS_DEFAULT: usize = 32;

/// 默认单步 input JSON 字节上限（env `FORGE_PLAN_MAX_INPUT_BYTES` 可调）。
pub const FORGE_PLAN_MAX_INPUT_BYTES_DEFAULT: usize = 65_536;

/// 结构校验（纯函数，零 IO）。冻结检查序：
/// 1. steps 非空
/// 2. step id 非空且唯一
/// 3. steps 数 ≤ 上限
/// 4. 逐步 input 序列化字节 ≤ 上限
/// 5. build_dag（自依赖/悬空依赖/环，复用 `graph::build_dag`）
pub fn validate_plan_structure(plan: &Plan) -> ForgeResult<()> {
    let max_steps = std::env::var("FORGE_PLAN_MAX_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORGE_PLAN_MAX_STEPS_DEFAULT);
    let max_input_bytes = std::env::var("FORGE_PLAN_MAX_INPUT_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORGE_PLAN_MAX_INPUT_BYTES_DEFAULT);

    // 1. steps 非空
    if plan.steps.is_empty() {
        return Err(ForgeError::InvalidState("plan: empty steps".into()));
    }

    // 2. step id 非空且唯一
    let mut seen: HashSet<&str> = HashSet::new();
    for step in &plan.steps {
        if step.id.is_empty() {
            return Err(ForgeError::InvalidState("plan: empty step id".into()));
        }
        if !seen.insert(step.id.as_str()) {
            return Err(ForgeError::InvalidState(format!(
                "plan: duplicate step id {}",
                step.id
            )));
        }
    }

    // 3. steps 数 ≤ 上限
    if plan.steps.len() > max_steps {
        return Err(ForgeError::InvalidState(format!(
            "plan: too many steps ({} > {})",
            plan.steps.len(),
            max_steps
        )));
    }

    // 4. 逐步 input 序列化字节 ≤ 上限
    for step in &plan.steps {
        if let forge_planner::StepAction::CallCapability { ref input, .. } = step.action {
            let bytes = serde_json::to_vec(input)
                .map_err(|e| ForgeError::InvalidState(format!("plan: serialize step input: {e}")))?;
            if bytes.len() > max_input_bytes {
                return Err(ForgeError::InvalidState(format!(
                    "plan: step {} input too large ({} > {})",
                    step.id,
                    bytes.len(),
                    max_input_bytes
                )));
            }
        }
    }

    // 5. build_dag（自依赖/悬空依赖/环）
    crate::graph::build_dag(plan)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{PlanId, TaskId};
    use forge_planner::{Plan, PlanStatus, PlanStep, StepAction};

    fn make_plan(steps: Vec<PlanStep>) -> Plan {
        Plan {
            id: PlanId::new_plan_id(),
            task_id: TaskId::new_task_id(),
            steps,
            status: PlanStatus::Ready,
        }
    }

    fn make_step(id: &str, deps: Vec<&str>) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: id.into(),
            depends_on: deps.into_iter().map(String::from).collect(),
            action: StepAction::CallCapability {
                capability: "cap".into(),
                input: serde_json::json!({}),
            },
        }
    }

    #[test]
    fn validate_plan_structure_accepts_linear_and_diamond() {
        // 线性
        let linear = make_plan(vec![
            make_step("s1", vec![]),
            make_step("s2", vec!["s1"]),
            make_step("s3", vec!["s2"]),
        ]);
        assert!(validate_plan_structure(&linear).is_ok());

        // 菱形
        let diamond = make_plan(vec![
            make_step("s1", vec![]),
            make_step("s2", vec!["s1"]),
            make_step("s3", vec!["s1"]),
            make_step("s4", vec!["s2", "s3"]),
        ]);
        assert!(validate_plan_structure(&diamond).is_ok());
    }

    #[test]
    fn rejects_empty_steps() {
        let plan = make_plan(vec![]);
        let err = validate_plan_structure(&plan).unwrap_err().to_string();
        assert!(err.contains("empty steps"));
    }

    #[test]
    fn rejects_duplicate_step_ids() {
        let plan = make_plan(vec![make_step("s1", vec![]), make_step("s1", vec![])]);
        let err = validate_plan_structure(&plan).unwrap_err().to_string();
        assert!(err.contains("duplicate step id"));
    }

    #[test]
    fn rejects_cycle() {
        let plan = make_plan(vec![
            make_step("s1", vec!["s2"]),
            make_step("s2", vec!["s1"]),
        ]);
        let err = validate_plan_structure(&plan).unwrap_err().to_string();
        assert!(err.contains("cycle"));
    }

    #[test]
    fn rejects_dangling_dependency() {
        let plan = make_plan(vec![make_step("s1", vec!["nonexistent"])]);
        let err = validate_plan_structure(&plan).unwrap_err().to_string();
        assert!(err.contains("dependency missing"));
    }

    #[test]
    fn rejects_oversized_plan() {
        let steps: Vec<PlanStep> = (0..33).map(|i| make_step(&format!("s{i}"), vec![])).collect();
        let plan = make_plan(steps);
        let err = validate_plan_structure(&plan).unwrap_err().to_string();
        assert!(err.contains("too many steps"));
    }
}
