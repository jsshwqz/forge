//! IMPROVE-9 路径 B：`forge_plan_execute` MCP 工具。
//!
//! 外部 Agent 提交自有 Plan JSON → Forge 校验（结构 + 能力白名单）→
//! 走既有 `run_end_to_end` CPEVR 闭环真实执行。
//! 步骤失败 → 既有 RecoveryStrategy 重试 → 仍败 → 升级（escalated_to_human=true），
//! 重规划责任在 Agent 侧（改计划后再次提交）。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult, PlanId, TaskId};
use forge_dag::validate_plan_structure;
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use forge_planner::{Plan, PlanStatus, PrefabPlanner, StepAction};
use forge_sdk::Orchestrator;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::orchestrate_tools::OrchestrateContext;

/// `forge_plan_execute` 工具描述（冻结）。
const TOOL_DESCRIPTION: &str = "Execute an agent-supplied plan for a task: validate structure + capability whitelist, then run the existing plan -> wave execute -> verify acceptance -> gate chain. Returns OrchestratorReport JSON. Replanning is the agent's responsibility: on failure the report carries escalated_to_human + failure detail; submit a revised plan in a new call.";

/// `forge_plan_execute` MCP 工具。
pub struct ForgePlanExecuteTool {
    ctx: Arc<OrchestrateContext>,
    descriptor: ToolDescriptor,
}

impl ForgePlanExecuteTool {
    pub fn new(ctx: Arc<OrchestrateContext>) -> Self {
        let input_schema = json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Task ID to execute the plan against" },
                "plan": {
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "title": { "type": "string" },
                                    "depends_on": { "type": "array", "items": { "type": "string" } },
                                    "action": { "oneOf": [
                                        { "type": "object", "properties": { "CallCapability": { "type": "object", "properties": { "capability": {"type":"string"}, "input": {} }, "required": ["capability","input"] } }, "required": ["CallCapability"] },
                                        { "type": "object", "properties": { "HumanApproval": { "type": "string" } }, "required": ["HumanApproval"] }
                                    ] }
                                },
                                "required": ["id", "title", "action"]
                            }
                        }
                    },
                    "required": ["steps"]
                },
                "workspace_task": { "type": "string", "description": "Optional prior task ID to reuse its workspace" }
            },
            "required": ["task_id", "plan"]
        });
        Self {
            ctx,
            descriptor: ToolDescriptor {
                name: "forge_plan_execute".into(),
                description: TOOL_DESCRIPTION.into(),
                input_schema,
                permission: PermissionLevel::WorkspaceWrite,
            },
        }
    }
}

#[async_trait]
impl Tool for ForgePlanExecuteTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        // ── 解析入参 ──
        let task_id_str = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ForgeError::InvalidState("forge_plan_execute: task_id required".into())
            })?;
        let task_id = TaskId::from(task_id_str.to_string());

        let plan_value = input
            .get("plan")
            .ok_or_else(|| ForgeError::InvalidState("forge_plan_execute: plan required".into()))?;

        let workspace_task = input
            .get("workspace_task")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // ── 反序列化 Plan.steps（depends_on 缺省 = []，忽略客户端 id/task_id/status）──
        let steps_raw = plan_value
            .get("steps")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ForgeError::InvalidState("forge_plan_execute: plan.steps array required".into())
            })?;
        let normalized_steps: Vec<Value> = steps_raw
            .iter()
            .map(|s| {
                let mut v = s.clone();
                if let Some(obj) = v.as_object_mut() {
                    obj.entry("depends_on").or_insert_with(|| json!([]));
                }
                v
            })
            .collect();
        let steps: Vec<forge_planner::PlanStep> =
            serde_json::from_value(Value::Array(normalized_steps)).map_err(|e| {
                ForgeError::InvalidState(format!("forge_plan_execute: bad plan: {e}"))
            })?;

        // ── 服务端接管字段（冻结）：客户端传的 id/task_id/status 一律忽略 ──
        let plan = Plan {
            id: PlanId::new_plan_id(),
            task_id: task_id.clone(),
            steps,
            status: PlanStatus::Ready,
        };

        // ── 校验链（顺序冻结，任一失败即返回，不进入执行）──

        // 1. 结构校验
        validate_plan_structure(&plan)?;

        // 2. HumanApproval 拒绝（自动模式不支持，A4）
        for step in &plan.steps {
            if let StepAction::HumanApproval(_) = step.action {
                return Err(ForgeError::InvalidState(format!(
                    "forge_plan_execute: HumanApproval not supported in auto mode (step {})",
                    step.id
                )));
            }
        }

        // 工作区
        let workdir = match &workspace_task {
            Some(prev) => self.ctx.workspace.create_for(prev.as_str())?,
            None => self.ctx.workspace.create_for(task_id.as_ref())?,
        };

        // 3. 能力白名单校验
        let available_caps: Vec<String> = OrchestrateContext::exec_router_for(&workdir)
            .list()
            .iter()
            .map(|d| d.name.clone())
            .collect();
        for step in &plan.steps {
            if let StepAction::CallCapability { ref capability, .. } = step.action {
                if !available_caps.contains(capability) {
                    let avail_list = available_caps.join(", ");
                    return Err(ForgeError::InvalidState(format!(
                        "forge_plan_execute: unknown capability '{}' (step {}); available: {}",
                        capability, step.id, avail_list
                    )));
                }
            }
        }

        // ── 装配（冻结）──
        // PrefabPlanner 注入；replanner = None（重规划责任在 Agent 侧）
        let planner: Option<Arc<dyn forge_planner::Planner>> =
            Some(Arc::new(PrefabPlanner::new(plan)));
        let deps = self.ctx.make_deps_for_full(
            &workdir,
            planner,
            None, // replanner = None
            0,    // max_replans = 0（无 replanner）
            workspace_task,
        );

        let orch = Orchestrator {
            capability: "echo".into(),
            timeout: self.ctx.timeout,
        };

        // ── 执行（run_end_to_end 一字不改）──
        let report = self.ctx.sdk.run_end_to_end(&task_id, &deps, &orch).await?;

        serde_json::to_value(&report).map_err(|e| {
            ForgeError::InvalidState(format!("forge_plan_execute: serialize report: {e}"))
        })
    }
}
