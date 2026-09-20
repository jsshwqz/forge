//! MCP-002：编排工具集 —— 把 plan→execute→verify 全链路暴露为 MCP tool。
//!
//! 外部 agent 通过这四个工具即可驱动 Forge 完整编排闭环：
//! 创建任务 → 查询任务 → 列举任务 → 端到端编排（计划/执行/验证/门禁）。
//!
//! 只调用 `ForgeSdk::run_end_to_end` 既有能力，不新增 Planner/Replanner/
//! 规划类型；存储后端跟随 `FORGE_PG_URL`（缺省回退内存栈）。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_evidence::InMemoryEvidenceStore;
use forge_exec::{
    PermissionLevel, PermissionPolicy, Tool, ToolDescriptor,
};
use forge_recovery::BoundedRetryStrategy;
use forge_sandbox::AllowListPolicy;
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_task::{AcceptanceCriterion, TaskStatus};
use crate::llm_wire;
use crate::planner::AcceptanceDrivenPlanner;
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// 编排工具名（注册白名单：出现在 `FORGE_TOOLS_BUILTIN` 才注册）。
pub const ORCHESTRATE_TOOLS: &[&str] = &[
    "forge_task_create",
    "forge_task_get",
    "forge_task_list",
    "forge_orchestrate",
];

/// 编排上下文：MCP-002 工具共享的运行时依赖。
///
/// 生命周期 = forge-mcp-server 进程生命周期；router 与 MCP server 主 router
/// 共享（编排执行时工具路由含全部已注册工具，计划只调度 base tools，无循环）。
pub struct OrchestrateContext {
    pub sdk: ForgeSdk,
    pub workspace: Arc<WorkspaceManager>,
    pub evidence: Arc<InMemoryEvidenceStore>,
    pub timeout: Duration,
}

impl OrchestrateContext {
    /// 构造编排上下文。存储后端跟随 FORGE_PG_URL；缺省内存栈。
    ///
    /// 工作区根：`FORGE_WORKSPACE` 优先，缺省系统临时目录。
    pub async fn new() -> ForgeResult<Self> {
        let sdk = match ForgeSdk::postgres_from_env().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("forge-mcp-server: PG unavailable, fallback in-memory ({e})");
                ForgeSdk::in_memory()
            }
        };
        let ws_root = std::env::var("FORGE_WORKSPACE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("forge-mcp-workspaces"));
        let workspace = Arc::new(WorkspaceManager::new(ws_root)?);
        Ok(Self {
            sdk,
            workspace,
            evidence: Arc::new(InMemoryEvidenceStore::default()),
            timeout: Duration::from_secs(300),
        })
    }

    /// 按任务 workdir 构造执行 router（文件工具 root=workdir，保证工具写文件
    /// 与验收核对在同一目录——Practice-1 实证 MCP-002 的 root 分裂缺陷）。
    fn exec_router_for(workdir: &std::path::Path) -> forge_exec::ToolRouter {
        use forge_exec::{EchoTool, WriteFileTool, ReadFileTool, ListDirTool, EditPatchTool};
        let r = forge_exec::ToolRouter::new();
        // 基础 5 工具（与 MCP-001 BASE_TOOLS 对齐），文件类以 workdir 为 root
        let _ = r.register(Box::new(EchoTool::new()));
        let _ = r.register(Box::new(WriteFileTool::new(workdir.to_path_buf())));
        let _ = r.register(Box::new(ReadFileTool::new(workdir.to_path_buf())));
        let _ = r.register(Box::new(ListDirTool::new(workdir.to_path_buf())));
        let _ = r.register(Box::new(EditPatchTool::new(workdir.to_path_buf())));
        // 纯解析工具（无 root 依赖）按 FORGE_TOOLS_BUILTIN 白名单附加
        let raw = std::env::var("FORGE_TOOLS_BUILTIN").unwrap_or_default();
        for name in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            // 已注册基础工具或编排/台账工具名跳过
            if r.route(name).is_ok() { continue; }
            if let Some(t) = crate::construct_tool(name, workdir) {
                let _ = r.register(t);
            }
        }
        r
    }

    /// 组装 OrchestratorDeps（每次调用新建，workdir 由调用方解析）。
    fn make_deps_for_full(
        &self,
        workdir: &std::path::Path,
        planner: Option<Arc<dyn forge_planner::Planner>>,
        replanner: Option<Arc<dyn forge_plan_llm::Replanner>>,
        max_replans: u32,
        workspace_task: Option<String>,
    ) -> OrchestratorDeps {
        // 策略：拒绝 Irreversible，其余放行（与 server DemoAllowAll 语义一致；
        // MCP-001 头注释已澄清 binary 侧不做 client 允许清单）
        let policy: Arc<dyn PermissionPolicy> = Arc::new(AllowListPolicy {
            allowed: vec![
                PermissionLevel::ReadOnly,
                PermissionLevel::WorkspaceWrite,
                PermissionLevel::External,
            ],
        });
        OrchestratorDeps {
            router: Arc::new(Self::exec_router_for(workdir)),
            policy,
            verifier_cmd: Arc::new(CommandVerifier),
            verifier_file: Arc::new(FileVerifier),
            evidence: self.evidence.clone(),
            workspace: self.workspace.clone(),
            timeout: self.timeout,
            // 与 server orchestrate 默认一致：有界重试 1 次，无 LLM 重规划器
            recovery: Arc::new(BoundedRetryStrategy {
                max_attempts: 1,
                base_backoff_ms: 200,
            }),
            replanner,
            max_replans,
            planner,
            workspace_task,
        }
    }
}

fn orch_desc(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
        permission: PermissionLevel::WorkspaceWrite,
    }
}

// ── forge_task_create ──

/// 创建任务（录入 goal/constraints/acceptance，不做执行）。
pub struct ForgeTaskCreateTool {
    ctx: Arc<OrchestrateContext>,
    descriptor: ToolDescriptor,
}

impl ForgeTaskCreateTool {
    fn new(ctx: Arc<OrchestrateContext>) -> Self {
        Self {
            ctx,
            descriptor: orch_desc(
                "forge_task_create",
                "Create a Forge task with goal, constraints and acceptance criteria. Returns task_id.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Task goal" },
                        "constraints": {
                            "type": "array", "items": { "type": "string" },
                            "description": "Optional constraints (default [])"
                        },
                        "acceptance": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "AcceptanceCriterion array: {id, description, check}. check is one of {\"Command\": cmd}, {\"FileExists\": path}, {\"FileContains\": {path, needle}}"
                        }
                    },
                    "required": ["name"]
                }),
            ),
        }
    }
}

#[async_trait]
impl Tool for ForgeTaskCreateTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        #[derive(Deserialize)]
        struct CreateInput {
            name: String,
            #[serde(default)]
            constraints: Vec<String>,
            #[serde(default)]
            acceptance: Vec<AcceptanceCriterion>,
        }
        let inp: CreateInput = serde_json::from_value(input)
            .map_err(|e| ForgeError::InvalidState(format!("forge_task_create: bad input: {e}")))?;
        let task = self
            .ctx
            .sdk
            .create_task(inp.name, inp.constraints, inp.acceptance)
            .await?;
        Ok(json!({ "task_id": task.id.to_string() }))
    }
}

// ── forge_task_get ──

/// 查询任务详情。
pub struct ForgeTaskGetTool {
    ctx: Arc<OrchestrateContext>,
    descriptor: ToolDescriptor,
}

impl ForgeTaskGetTool {
    fn new(ctx: Arc<OrchestrateContext>) -> Self {
        Self {
            ctx,
            descriptor: orch_desc(
                "forge_task_get",
                "Get task details by task_id (goal, status, constraints, acceptance).",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task ID" }
                    },
                    "required": ["task_id"]
                }),
            ),
        }
    }
}

#[async_trait]
impl Tool for ForgeTaskGetTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("forge_task_get: task_id required".into()))?;
        let id = TaskId::from(task_id.to_string());
        let task = self.ctx.sdk.get_task(&id).await?;
        Ok(serde_json::to_value(&task)
            .map_err(|e| ForgeError::InvalidState(format!("forge_task_get: serialize: {e}")))?)
    }
}

// ── forge_task_list ──

/// 列出任务 ID（可选按状态过滤）。
pub struct ForgeTaskListTool {
    ctx: Arc<OrchestrateContext>,
    descriptor: ToolDescriptor,
}

impl ForgeTaskListTool {
    fn new(ctx: Arc<OrchestrateContext>) -> Self {
        Self {
            ctx,
            descriptor: orch_desc(
                "forge_task_list",
                "List task IDs, optionally filtered by status (Pending/Planned/Executing/Verifying/Completed/Failed).",
                json!({
                    "type": "object",
                    "properties": {
                        "status": {
                            "type": "string",
                            "description": "Optional status filter"
                        }
                    }
                }),
            ),
        }
    }
}

#[async_trait]
impl Tool for ForgeTaskListTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let status_filter: Option<TaskStatus> = match input.get("status") {
            None | Some(Value::Null) => None,
            Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
                ForgeError::InvalidState(format!("forge_task_list: bad status: {e}"))
            })?),
        };
        let ids = self.ctx.sdk.list_tasks().await?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(sf) = &status_filter {
                if let Ok(task) = self.ctx.sdk.get_task(&id).await {
                    if &task.status != sf {
                        continue;
                    }
                }
            }
            out.push(id.to_string());
        }
        Ok(json!({ "task_ids": out }))
    }
}

// ── forge_orchestrate ──

/// 端到端编排：plan → execute → verify → gate → 状态迁移。
pub struct ForgeOrchestrateTool {
    ctx: Arc<OrchestrateContext>,
    descriptor: ToolDescriptor,
}

impl ForgeOrchestrateTool {
    fn new(ctx: Arc<OrchestrateContext>) -> Self {
        Self {
            ctx,
            descriptor: orch_desc(
                "forge_orchestrate",
                "Run end-to-end orchestration for a task: plan -> wave execute -> verify acceptance -> gate -> status. Returns OrchestratorReport JSON.",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task ID to orchestrate" },
                        "max_replans": {
                            "type": "integer",
                            "description": "Replan budget on step failure (default 1)"
                        },
                        "workspace_task": {
                            "type": "string",
                            "description": "Optional prior task ID to reuse its workspace"
                        }
                    },
                    "required": ["task_id"]
                }),
            ),
        }
    }
}

#[async_trait]
impl Tool for ForgeOrchestrateTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("forge_orchestrate: task_id required".into()))?;
        let id = TaskId::from(task_id.to_string());
        let max_replans = input
            .get("max_replans")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .min(10) as u32;
        let workspace_task = input
            .get("workspace_task")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // MCP-003 票2：工作区对齐——先 create_for 拿 workdir，再以 workdir 构造
        // 执行 router（工具 root 与验收 workdir 同一目录）
        let workdir = match &workspace_task {
            Some(prev) => self.ctx.workspace.create_for(prev.as_str())?,
            None => self.ctx.workspace.create_for(id.as_ref())?,
        };
        // MCP-004：有 LLM 配置 → LLM 多步规划（按 goal 自然语言生成）；
        // 无配置 → MCP-003 验收驱动规划器（离线零回归）
        let exec_tools: Vec<String> = OrchestrateContext::exec_router_for(&workdir)
            .list()
            .iter()
            .map(|d| d.name.clone())
            .collect::<Vec<_>>();
        let (planner, replanner): (
            Option<Arc<dyn forge_planner::Planner>>,
            Option<Arc<dyn forge_plan_llm::Replanner>>,
        ) = if llm_wire::llm_configured() {
            match llm_wire::wire_from_env(exec_tools) {
                Some(w) => (Some(w.planner), Some(w.replanner)),
                None => (
                    Some(Arc::new(AcceptanceDrivenPlanner::default())),
                    None,
                ),
            }
        } else {
            (
                Some(Arc::new(AcceptanceDrivenPlanner::default())),
                None,
            )
        };
        let deps = self.ctx.make_deps_for_full(
            &workdir, planner, replanner, max_replans, workspace_task
        );
        let orch = Orchestrator {
            capability: "echo".into(),
            timeout: self.ctx.timeout,
        };
        let report = self.ctx.sdk.run_end_to_end(&id, &deps, &orch).await?;
        serde_json::to_value(&report).map_err(|e| {
            ForgeError::InvalidState(format!("forge_orchestrate: serialize report: {e}"))
        })
    }
}

/// 构造编排工具（注册白名单点名时调用）。
pub fn construct_orchestrate_tool(
    name: &str,
    ctx: &Arc<OrchestrateContext>,
) -> Option<Box<dyn Tool>> {
    match name {
        "forge_task_create" => Some(Box::new(ForgeTaskCreateTool::new(ctx.clone()))),
        "forge_task_get" => Some(Box::new(ForgeTaskGetTool::new(ctx.clone()))),
        "forge_task_list" => Some(Box::new(ForgeTaskListTool::new(ctx.clone()))),
        "forge_orchestrate" => Some(Box::new(ForgeOrchestrateTool::new(ctx.clone()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_get_list_roundtrip_in_memory() {
        let ctx = OrchestrateContext::new().await.unwrap();
        let tool = ForgeTaskCreateTool::new(Arc::new(ctx));

        let out = tool
            .invoke(json!({
                "name": "hello",
                "acceptance": [{
                    "id": "AC-1",
                    "description": "noop ok",
                    "check": { "Command": "true" }
                }]
            }))
            .await
            .unwrap();
        let task_id = out["task_id"].as_str().unwrap().to_string();

        let get = ForgeTaskGetTool::new(tool.ctx.clone());
        let detail = get.invoke(json!({ "task_id": task_id })).await.unwrap();
        assert_eq!(detail["goal"], "hello");

        let list = ForgeTaskListTool::new(tool.ctx.clone());
        let ids = list.invoke(json!({})).await.unwrap();
        assert!(ids["task_ids"].as_array().unwrap().contains(&json!(task_id)));

        // status 过滤：新任务 Pending
        let pending = list.invoke(json!({ "status": "Pending" })).await.unwrap();
        assert!(pending["task_ids"].as_array().unwrap().contains(&json!(task_id)));
        let completed = list.invoke(json!({ "status": "Completed" })).await.unwrap();
        assert!(!completed["task_ids"].as_array().unwrap().contains(&json!(task_id)));
    }

    #[tokio::test]
    async fn orchestrate_end_to_end_completes() {
        let ctx = OrchestrateContext::new().await.unwrap();
        let ctx = Arc::new(ctx);
        let create = ForgeTaskCreateTool::new(ctx.clone());
        let task_id = create
            .invoke(json!({
                "name": "orchestrate me",
                "acceptance": [{
                    "id": "AC-1",
                    "description": "command true passes",
                    "check": { "Command": "true" }
                }]
            }))
            .await
            .unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_string();

        let orch = ForgeOrchestrateTool::new(ctx.clone());
        let report = orch.invoke(json!({ "task_id": task_id })).await.unwrap();
        assert_eq!(report["final_status"], "Completed");
        assert_eq!(report["gate"]["passed"], true);
        assert!(!report["plan_versions"].as_array().unwrap().is_empty());
    }
}