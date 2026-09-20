//! MCP-003 票1：验收驱动规划器（AcceptanceDrivenPlanner）。
//!
//! 修 MCP-002 缺陷：编排规划器写死 echo，不按验收选工具，
//! 导致"写文件"任务必然 Failed（Practice-1 实证）。
//!
//! 策略：按 task.acceptance 的 CheckSpec 派生计划步骤——
//! - FileExists(path)     → 前置 write_file(path, content=task.goal) 步骤
//! - FileContains{path,..}→ 前置 write_file(path, content=needle) 步骤
//! - Command(_)           → echo 占位（与 MCP-002 现状一致，不破坏）
//!
//! 步骤 depends_on 串链（与 SequentialPlanner 同风格）。
//! 不新增依赖：实现既有 forge_planner::Planner trait。

use forge_planner::{Plan, PlanStatus, PlanStep, Planner, StepAction};
use forge_task::{CheckSpec, Task};
use forge_core::{ForgeResult, PlanId};

/// 验收驱动规划器。
#[derive(Clone, Debug)]
pub struct AcceptanceDrivenPlanner {
    /// 写文件工具名（router 中注册名）。
    pub write_tool: String,
    /// 无验收/Command 验收时的兜底能力（保持 echo 兼容）。
    pub fallback_capability: String,
}

impl Default for AcceptanceDrivenPlanner {
    fn default() -> Self {
        Self {
            write_tool: "write_file".into(),
            fallback_capability: "echo".into(),
        }
    }
}

#[async_trait::async_trait]
impl Planner for AcceptanceDrivenPlanner {
    async fn plan(&self, task: &Task) -> ForgeResult<Plan> {
        let mut steps: Vec<PlanStep> = Vec::new();
        if task.acceptance.is_empty() {
            // 无验收 → 与 SequentialPlanner 一致：单个"执行目标"步骤
            steps.push(PlanStep {
                id: "step_1".into(),
                title: format!("执行目标: {}", task.goal),
                depends_on: vec![],
                action: StepAction::CallCapability {
                    capability: self.fallback_capability.clone(),
                    input: serde_json::json!({ "goal": task.goal }),
                },
            });
            return Ok(Plan {
                id: PlanId::new_plan_id(),
                task_id: task.id.clone(),
                steps,
                status: PlanStatus::Ready,
            });
        }

        let mut prev_id: Option<String> = None;
        for (i, ac) in task.acceptance.iter().enumerate() {
            let step_id = format!("step_{}", i + 1);
            let depends_on = match &prev_id {
                Some(pid) => vec![pid.clone()],
                None => vec![],
            };
            let action = match &ac.check {
                // 文件类验收 → 前置写文件步骤（真实干活）
                CheckSpec::FileExists(path) => StepAction::CallCapability {
                    capability: self.write_tool.clone(),
                    input: serde_json::json!({
                        "path": path,
                        "content": task.goal,
                    }),
                },
                CheckSpec::FileContains { path, needle } => StepAction::CallCapability {
                    capability: self.write_tool.clone(),
                    input: serde_json::json!({
                        "path": path,
                        "content": needle,
                    }),
                },
                // 命令验收 → 保持 echo 占位（不破坏既有行为）
                CheckSpec::Command(_) => StepAction::CallCapability {
                    capability: self.fallback_capability.clone(),
                    input: serde_json::json!({
                        "criterion_id": ac.id,
                        "description": ac.description,
                    }),
                },
            };
            steps.push(PlanStep {
                id: step_id.clone(),
                title: format!("验收: {} - {}", ac.id, ac.description),
                depends_on,
                action,
            });
            prev_id = Some(step_id);
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
    use forge_task::{AcceptanceCriterion, Task};

    fn make_task(acceptance: Vec<AcceptanceCriterion>) -> Task {
        Task::new(TaskId::new_task_id(), "write out.txt via forge".into(), vec![], acceptance)
    }

    fn ac_file_exists(path: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: "AC-1".into(),
            description: "out exists".into(),
            check: CheckSpec::FileExists(path.into()),
        }
    }

    #[tokio::test]
    async fn file_exists_plans_write_step() {
        let p = AcceptanceDrivenPlanner::default();
        let plan = p.plan(&make_task(vec![ac_file_exists("out.txt")])).await.unwrap();
        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0].action {
            StepAction::CallCapability { capability, input } => {
                assert_eq!(capability, "write_file");
                assert_eq!(input["path"], "out.txt");
                assert_eq!(input["content"], "write out.txt via forge");
            }
            _ => panic!("expected CallCapability"),
        }
    }
}