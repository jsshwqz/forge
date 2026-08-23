//! 顺序规划器。
//!
//! 把 task.acceptance 每条展开为一个顺序 step，
//! 无验收标准的任务生成单个"执行目标"step；depends_on 串成链。

use crate::model::{Plan, PlanStatus, PlanStep, Planner, StepAction};
use async_trait::async_trait;
use forge_core::{ForgeResult, PlanId};

/// 顺序规划器。
pub struct SequentialPlanner {
    /// 能力名称。
    pub capability: String,
}

#[async_trait]
impl Planner for SequentialPlanner {
    async fn plan(&self, task: &forge_task::Task) -> ForgeResult<Plan> {
        let mut steps = Vec::new();
        let mut prev_id: Option<String> = None;

        if task.acceptance.is_empty() {
            // 无验收标准 → 单个"执行目标"step
            let step_id = "step_1".to_string();
            steps.push(PlanStep {
                id: step_id.clone(),
                title: format!("执行目标: {}", task.goal),
                depends_on: vec![],
                action: StepAction::CallCapability {
                    capability: self.capability.clone(),
                    input: serde_json::json!({"goal": task.goal}),
                },
            });
        } else {
            // 每条验收标准展开为一个 step
            for (i, ac) in task.acceptance.iter().enumerate() {
                let step_id = format!("step_{}", i + 1);
                let depends_on = match &prev_id {
                    Some(pid) => vec![pid.clone()],
                    None => vec![],
                };
                steps.push(PlanStep {
                    id: step_id.clone(),
                    title: format!("验收: {} - {}", ac.id, ac.description),
                    depends_on,
                    action: StepAction::CallCapability {
                        capability: self.capability.clone(),
                        input: serde_json::json!({
                            "criterion_id": ac.id,
                            "description": ac.description,
                        }),
                    },
                });
                prev_id = Some(step_id);
            }
        }

        Ok(Plan {
            id: PlanId::new_plan_id(),
            task_id: task.id.clone(),
            steps,
            status: PlanStatus::Ready,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::TaskId;
    use forge_task::{AcceptanceCriterion, CheckSpec, Task};

    fn make_task(acceptance: Vec<AcceptanceCriterion>) -> Task {
        Task::new(
            TaskId::new_task_id(),
            "test goal".into(),
            vec![],
            acceptance,
        )
    }

    fn make_ac(id: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: id.into(),
            description: "test criterion".into(),
            check: CheckSpec::FileExists("output.txt".into()),
        }
    }

    #[tokio::test]
    async fn test_three_acceptance_three_steps() {
        let planner = SequentialPlanner {
            capability: "test_cap".into(),
        };
        let task = make_task(vec![make_ac("AC-1"), make_ac("AC-2"), make_ac("AC-3")]);
        let plan = planner.plan(&task).await.unwrap();

        assert_eq!(plan.steps.len(), 3);
        // 链状依赖
        assert!(plan.steps[0].depends_on.is_empty());
        assert_eq!(plan.steps[1].depends_on, vec!["step_1"]);
        assert_eq!(plan.steps[2].depends_on, vec!["step_2"]);
    }

    #[tokio::test]
    async fn test_no_acceptance_single_step() {
        let planner = SequentialPlanner {
            capability: "test_cap".into(),
        };
        let task = make_task(vec![]);
        let plan = planner.plan(&task).await.unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].depends_on.is_empty());
    }

    #[tokio::test]
    async fn test_deterministic_same_input() {
        let planner = SequentialPlanner {
            capability: "test_cap".into(),
        };
        let task = make_task(vec![make_ac("AC-1"), make_ac("AC-2")]);

        let plan1 = planner.plan(&task).await.unwrap();
        let plan2 = planner.plan(&task).await.unwrap();

        // 结构相同（step 数量、依赖关系确定）
        assert_eq!(plan1.steps.len(), plan2.steps.len());
        for (s1, s2) in plan1.steps.iter().zip(plan2.steps.iter()) {
            assert_eq!(s1.id, s2.id);
            assert_eq!(s1.depends_on, s2.depends_on);
            assert_eq!(s1.title, s2.title);
        }
    }
}
