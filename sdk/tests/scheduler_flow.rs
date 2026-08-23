//! SDK-002：planner → DAG → scheduler 链路经 SDK 的集成验证。
//!
//! 证明 SDK 句柄可以驱动"任务→计划→波次执行"完整编排
//! （echo 能力作为 StepExecutor 的最小实现）。

use forge_dag::build_dag;
use forge_planner::{Planner, StepId};
use forge_scheduler::{run_plan, RunSummary, StepExecutor};
use forge_sdk::ForgeSdk;
use forge_task::AcceptanceCriterion;

/// Echo 步骤执行器：把步骤输入原样返回（最小可用编排目标）。
struct EchoStepExecutor;

#[async_trait::async_trait]
impl StepExecutor for EchoStepExecutor {
    async fn execute(
        &self,
        step_id: &StepId,
        action: &forge_planner::StepAction,
    ) -> forge_core::ForgeResult<serde_json::Value> {
        match action {
            forge_planner::StepAction::CallCapability { capability, input } => Ok(
                serde_json::json!({"step": step_id, "capability": capability, "echo": input}),
            ),
            forge_planner::StepAction::HumanApproval(msg) => Ok(
                serde_json::json!({"step": step_id, "approval": msg}),
            ),
        }
    }
}

#[tokio::test]
async fn sdk_plan_and_schedule_flow() {
    let sdk = ForgeSdk::in_memory();

    // 1) 建任务：3 条验收标准 → SequentialPlanner 展开为 3 个链式步骤
    let task = sdk
        .create_task(
            "三步串行任务",
            vec![],
            vec![
                ac("AC-1"), ac("AC-2"), ac("AC-3"),
            ],
        )
        .await
        .unwrap();

    // 2) 规划（SDK 暴露 planner 的最小方式：直接构造，capability 复用 echo）
    let planner = forge_planner::SequentialPlanner { capability: "echo".into() };
    let plan = planner.plan(&task).await.unwrap();
    assert_eq!(plan.steps.len(), 3);

    // 3) DAG 构建与波次调度
    let dag = build_dag(&plan).unwrap();
    let summary: RunSummary = run_plan(&dag, &plan, &EchoStepExecutor).await.unwrap();

    // 链式依赖 → 3 波次顺序完成
    assert!(summary.succeeded());
    assert_eq!(summary.waves, 3);
    assert_eq!(
        summary.completed,
        vec!["step_1".to_string(), "step_2".to_string(), "step_3".to_string()]
    );

    // 4) 会话记录存在性（任务可追溯）
    let session = sdk.create_session(task.id.clone()).await.unwrap();
    assert_eq!(session.task_id, task.id);
}

fn ac(id: &str) -> AcceptanceCriterion {
    AcceptanceCriterion {
        id: id.into(),
        description: format!("criterion {id}"),
        check: forge_task::CheckSpec::FileExists(format!("{id}.txt")),
    }
}
