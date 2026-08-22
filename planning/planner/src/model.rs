//! Plan 模型定义。

use async_trait::async_trait;
use forge_core::{ForgeResult, PlanId, TaskId};
use serde::{Deserialize, Serialize};

/// 计划状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStatus {
    Draft,
    Ready,
    Executing,
    Done,
    Failed,
}

/// Step ID，如 "step_1"、"step_2"。
pub type StepId = String;

/// 计划步骤。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanStep {
    /// 步骤 ID。
    pub id: StepId,
    /// 标题。
    pub title: String,
    /// 依赖的步骤 ID 列表。
    pub depends_on: Vec<StepId>,
    /// 步骤动作。
    pub action: StepAction,
}

/// 步骤动作。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StepAction {
    /// 调用能力。
    CallCapability {
        /// 能力名称。
        capability: String,
        /// 输入。
        input: serde_json::Value,
    },
    /// 人工审批。
    HumanApproval(String),
}

/// 计划对象。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    /// 计划 ID。
    pub id: PlanId,
    /// 关联的任务 ID。
    pub task_id: TaskId,
    /// 步骤列表。
    pub steps: Vec<PlanStep>,
    /// 计划状态。
    pub status: PlanStatus,
}

/// 规划器 trait。
#[async_trait]
pub trait Planner: Send + Sync {
    /// 为任务生成计划。
    async fn plan(&self, task: &forge_task::Task) -> ForgeResult<Plan>;
}
