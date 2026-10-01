//! 预制计划适配器（IMPROVE-9 路径 B）：Agent 侧产出的 Plan 经校验后由此插入既有编排链。

use crate::model::{Plan, Planner};
use async_trait::async_trait;

/// 预制计划适配器：持有已校验的 Plan，`plan()` 原样返回。
///
/// 用法：外部 Agent 提交 Plan JSON → `validate_plan_structure` 校验通过 →
/// `PrefabPlanner::new(plan)` 注入 `OrchestratorDeps.planner` →
/// `run_end_to_end` 走既有 CPEVR 闭环。
pub struct PrefabPlanner {
    plan: Plan,
}

impl PrefabPlanner {
    pub fn new(plan: Plan) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl Planner for PrefabPlanner {
    async fn plan(&self, _task: &forge_task::Task) -> forge_core::ForgeResult<Plan> {
        Ok(self.plan.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Plan, PlanStatus, PlanStep, StepAction};
    use forge_core::{PlanId, TaskId};

    fn dummy_plan() -> Plan {
        Plan {
            id: PlanId::new_plan_id(),
            task_id: TaskId::new_task_id(),
            steps: vec![PlanStep {
                id: "s1".into(),
                title: "step 1".into(),
                depends_on: vec![],
                action: StepAction::CallCapability {
                    capability: "echo".into(),
                    input: serde_json::json!({}),
                },
            }],
            status: PlanStatus::Ready,
        }
    }

    #[tokio::test]
    async fn prefab_planner_returns_plan_clone() {
        let plan = dummy_plan();
        let expected_id = plan.id.clone();
        let expected_steps = plan.steps.len();
        let prefab = PrefabPlanner::new(plan);
        let task = forge_task::Task::new(
            TaskId::new_task_id(),
            "test goal".into(),
            vec![],
            vec![],
        );
        let result = prefab.plan(&task).await.unwrap();
        assert_eq!(result.id, expected_id);
        assert_eq!(result.steps.len(), expected_steps);
    }
}
