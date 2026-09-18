//! forge-server：Aion Forge 2.0 HTTP API 层（V3.0 完整实现 + V4.0 产品工厂）。
//!
//! 端点：health / tasks CRUD / sessions / orchestrate / evidence / events/stream
//!       /templates /products（生命周期）/ metrics / 控制台三页
//! 安全基线（SEC-001）：Bearer 鉴权（FORGE_API_KEY）、CORS 默认关闭、
//!       非 loopback 监听未配 key 拒绝启动。

pub mod auth;
pub mod billing;
pub mod bus;
pub mod queue;
pub mod mcp_tools;
pub mod sandbox_verify;
pub mod quota;
pub mod routes;
pub mod sse_relay;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event as SseEvent, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use auth::AuthConfig;
use forge_core::{ForgeError, ForgeResult, SessionId, TaskId};
use forge_evidence::{EvidenceStore, InMemoryEvidenceStore};
use forge_event::{EventBus, InMemoryEventBus, Topic};
use forge_exec::{EchoTool, PermissionPolicy, ToolRouter};
use forge_session::SessionStore;
use forge_sdk::{ForgeSdk, Orchestrator};
use forge_task::{AcceptanceCriterion, Task, TaskStatus, TaskStore};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use forge_product_instance::{
    ProductInstanceStore as _, TemplateRegistry as _,
};
use forge_cap::InMemoryCapabilityRegistry;
use forge_knowledge::{FailureKnowledgeBase as _, InMemoryKnowledgeBase, KnowledgeEntry, ReplayArchive};
use forge_recovery::classify::FailureCategory;
use futures::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

// ==================== AppState ====================

/// 运行时计数器（OBS-002，手写 Prometheus 文本格式）。
#[derive(Default)]
pub struct Metrics {
    pub tasks_total: std::sync::atomic::AtomicU64,
    pub executions_total: std::sync::atomic::AtomicU64,
    pub verifications_pass: std::sync::atomic::AtomicU64,
    pub verifications_fail: std::sync::atomic::AtomicU64,
    pub replans_total: std::sync::atomic::AtomicU64,
}

impl Metrics {
    /// 渲染 Prometheus 文本暴露格式（零依赖，契约 6.2）。
    pub fn render(&self) -> String {
        use std::sync::atomic::Ordering;
        let g = |c: &std::sync::atomic::AtomicU64| c.load(Ordering::Relaxed).to_string();
        format!(
            "# HELP tasks_total Total tasks created.\n\
             # TYPE tasks_total counter\n\
             tasks_total {tasks}\n\
             # HELP executions_total Total orchestrate runs.\n\
             # TYPE executions_total counter\n\
             executions_total {exec}\n\
             # HELP verifications_pass Acceptance checks passed.\n\
             # TYPE verifications_pass counter\n\
             verifications_pass {vp}\n\
             # HELP verifications_fail Acceptance checks failed.\n\
             # TYPE verifications_fail counter\n\
             verifications_fail {vf}\n\
             # HELP replans_total Replan attempts consumed (ORCH-003).\n\
             # TYPE replans_total counter\n\
             replans_total {rp}\n",
            tasks = g(&self.tasks_total),
            exec = g(&self.executions_total),
            vp = g(&self.verifications_pass),
            vf = g(&self.verifications_fail),
            rp = g(&self.replans_total),
        )
    }
}

/// DEP-001 D2：SSE 广播缓冲容量（`FORGE_SSE_BUFFER`，默认 1024，与 docs/SCALING.md 一致）。
fn sse_buffer() -> usize {
    std::env::var("FORGE_SSE_BUFFER")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1024)
}

#[derive(Clone)]
pub struct AppState {
    pub sdk: ForgeSdk,
    pub evidence: Arc<InMemoryEvidenceStore>,
    pub workspaces: Arc<WorkspaceManager>,
    pub event_bus: Arc<InMemoryEventBus>,
    pub instances: Arc<forge_product_instance::InMemoryProductInstanceStore>,
    pub templates: Arc<forge_product_instance::InMemoryTemplateRegistry>,
    pub metrics: Arc<Metrics>,
    /// KNW-001：失败知识库（服务面 GA-FIX-2）。
    pub knowledge: Arc<InMemoryKnowledgeBase>,
    /// V5.0 MKT：能力注册表（市场源）。
    pub capabilities: Arc<InMemoryCapabilityRegistry>,
    /// V5.0 TEN-002：鉴权配置与租户钥存储（V5-FIX-2a 接线）。
    pub auth: AuthConfig,
    pub tenant_keys: Arc<dyn auth::TenantKeyStore>,
    /// V5.0 TEN-003：租户配额存储（V5-FIX-2b 接线）。
    pub quotas: Arc<dyn quota::QuotaStore>,
    /// V6.0 FED-001：PG 连接池（内存模式为 None，队列路径由此门控）。
    pub pool: Option<sqlx::PgPool>,
    /// 大模型运行时配置（支持页面热更新与持久化）。
    pub llm_config: Arc<tokio::sync::RwLock<routes::llm::LlmRuntimeConfig>>,
}

impl AppState {
    pub fn in_memory() -> Self {
        Self {
            sdk: ForgeSdk::in_memory(),
            evidence: Arc::new(InMemoryEvidenceStore::default()),
            workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
            event_bus: Arc::new(InMemoryEventBus::with_buffer(sse_buffer())),
            instances: Arc::new(Default::default()),
            templates: Arc::new(Default::default()),
            metrics: Arc::new(Metrics::default()),
            knowledge: Arc::new(Default::default()),
            capabilities: Arc::new(Default::default()),
            auth: AuthConfig::from_env(),
            tenant_keys: Arc::new(auth::InMemoryTenantKeyStore::default()),
            quotas: Arc::new(quota::InMemoryQuotaStore::default()),
            pool: None,
            llm_config: Arc::new(tokio::sync::RwLock::new(routes::llm::LlmRuntimeConfig::from_env())),
        }
    }
    pub fn new(tasks: Arc<dyn TaskStore>, sessions: Arc<dyn SessionStore>) -> Self {
        Self {
            sdk: ForgeSdk::from_stores(tasks, sessions),
            evidence: Arc::new(InMemoryEvidenceStore::default()),
            workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
            event_bus: Arc::new(InMemoryEventBus::with_buffer(sse_buffer())),
            instances: Arc::new(Default::default()),
            templates: Arc::new(Default::default()),
            metrics: Arc::new(Metrics::default()),
            knowledge: Arc::new(Default::default()),
            capabilities: Arc::new(Default::default()),
            auth: AuthConfig::from_env(),
            tenant_keys: Arc::new(auth::InMemoryTenantKeyStore::default()),
            quotas: Arc::new(quota::InMemoryQuotaStore::default()),
            pool: None,
            llm_config: Arc::new(tokio::sync::RwLock::new(routes::llm::LlmRuntimeConfig::from_env())),
        }
    }
}

// ==================== 错误 ====================

pub struct ApiError(StatusCode, String);

impl From<ForgeError> for ApiError {
    fn from(e: ForgeError) -> Self {
        let status = match &e {
            ForgeError::NotFound(_) => StatusCode::NOT_FOUND,
            // TEN-004（R2）：PG 租户态不可用 → 503 显式失败，禁止静默空数据
            ForgeError::InvalidState(msg) if auth::is_pg_store_unavailable(&e) => {
                let _ = msg;
                return ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    serde_json::json!({ "error": { "code": "storage_unavailable" } }).to_string(),
                );
            }
            // TEN-003（V5-FIX-2b）：本服务配额超限 → 429 + 冻结错误体
            ForgeError::InvalidState(msg)
                if msg.starts_with("quota_concurrency") || msg.starts_with("quota_daily") =>
            {
                let code = msg.split(':').next().unwrap_or("quota_exceeded");
                return ApiError(
                    StatusCode::TOO_MANY_REQUESTS,
                    serde_json::json!({ "error": { "code": code } }).to_string(),
                );
            }
            ForgeError::InvalidState(msg)
                if msg.contains("llm http 429")
                    || msg.contains("insufficient_quota")
                    || msg.contains("quota") =>
            {
                // 上游供应商配额/限流：语义上是服务暂不可用，而非请求冲突
                StatusCode::SERVICE_UNAVAILABLE
            }
            ForgeError::InvalidState(_) => StatusCode::CONFLICT,
            ForgeError::PermissionDenied(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(status, e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 已在 From<ForgeError> 预构造的错误体（storage_unavailable 503 /
        // quota 429 的冻结错误体 {"error":{"code":...}}）直接透传，避免二次包装。
        // 普通 ForgeError 的 payload 是描述文本，走统一 {status,message} 包装。
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.1) {
            if v.get("error").is_some() {
                return (self.0, axum::Json(v)).into_response();
            }
        }
        let body = serde_json::json!({
            "error": {
                "status": self.0.as_u16(),
                "message": self.1,
            }
        });
        (self.0, axum::Json(body)).into_response()
    }
}

// ==================== 请求体 ====================

#[derive(Deserialize)]
pub struct CreateTaskRequest {
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
}

#[derive(Deserialize)]
pub struct OrchestrateRequest {
    pub goal: String,
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// V5.1 CGN-001：`true`（默认，保持既有行为）时 LLM 路径选用单文件代码生成
    /// 规划器；`false` 回退确定性顺序规划。
    #[serde(default = "default_codegen_flag")]
    pub codegen_flag: bool,
    /// V7 ORCH-101a：规划模式（缺省 auto = 完全兼容既有行为）。
    #[serde(default)]
    pub plan_mode: Option<PlanMode>,
    /// V8 CTX-001：续作既有任务工作区（增量开发入口）。None = 新建本任务工作区。
    #[serde(default)]
    pub workspace_task_id: Option<String>,
}
fn default_timeout() -> u64 { 30 }
fn default_codegen_flag() -> bool { true }

/// 规划器选择结果：（MultiStep 时的）重规划器可选项。
type PlannerOpt = Option<Arc<dyn forge_planner::Planner>>;
type ReplannerOpt = Option<Arc<dyn forge_plan_llm::Replanner>>;

/// V7 ORCH-101a：规划模式（R1 冻结——显式 plan_mode > codegen_flag；Auto 恒等于
/// Codegen 保证既有客户端零破坏）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    Auto,
    Codegen,
    MultiStep,
}

/// 规划模式解析（冻结纯函数，离线可测）。
pub fn resolve_plan_mode(plan_mode: Option<PlanMode>, codegen_flag: bool) -> PlanMode {
    match plan_mode {
        Some(PlanMode::MultiStep) => PlanMode::MultiStep,
        // R1：Auto 恒等于 Codegen（零破坏承诺）；None 才跟随 codegen_flag
        Some(PlanMode::Codegen) | Some(PlanMode::Auto) => PlanMode::Codegen,
        None => {
            if codegen_flag { PlanMode::Codegen } else { PlanMode::MultiStep }
        }
    }
}

pub struct DemoAllowAll;
impl PermissionPolicy for DemoAllowAll {
    fn check(&self, _: forge_exec::PermissionLevel, _: &forge_exec::PolicyContext) -> ForgeResult<()> { Ok(()) }
}

// ==================== Handlers ====================

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok","service":"forge-server","version":env!("CARGO_PKG_VERSION")}))
}

async fn create_task(State(st): State<AppState>, Json(req): Json<CreateTaskRequest>) -> Result<Json<Task>, ApiError> {
    use std::sync::atomic::Ordering;
    st.metrics.tasks_total.fetch_add(1, Ordering::Relaxed);
    Ok(Json(st.sdk.create_task(req.goal, req.constraints, req.acceptance).await?))
}

async fn list_tasks(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
) -> Json<serde_json::Value> {
    // V5-FIX-2d：有租户身份时按租户域列举
    let ids = match outcome.0 {
        auth::AuthOutcome::Tenant(t) => st.sdk.tasks().list_in_tenant(&t).await,
        auth::AuthOutcome::Local => st.sdk.list_tasks().await,
    }
    .unwrap_or_default();
    Json(serde_json::json!({"count": ids.len(), "ids": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>()}))
}

async fn get_task(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Path(id): Path<String>,
) -> Result<Json<Task>, ApiError> {
    let tid = TaskId::from(id);
    // V5-FIX-2d：有租户身份时按租户域读取（跨租户 → PermissionDenied/403）
    let task = match outcome.0 {
        auth::AuthOutcome::Tenant(t) => st.sdk.tasks().get_in_tenant(&t, &tid).await?,
        auth::AuthOutcome::Local => st.sdk.get_task(&tid).await?,
    };
    Ok(Json(task))
}

async fn get_session(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<forge_session::Session>, ApiError> {
    Ok(Json(st.sdk.sessions().get(&SessionId::from(id)).await?))
}

/// 执行一次编排（FED-001 R4 抽取：HTTP 直连路径与队列 worker 共用同一执行体）。
async fn execute_orchestration(
    st: &AppState,
    task: &Task,
    timeout_secs: u64,
    plan_mode: PlanMode,
    tenant: &str,
    workspace_task_id: Option<&str>,
) -> Result<forge_sdk::OrchestratorReport, ApiError> {
    // 工具集：echo(基线) + write_file("写软件"落盘能力，根=任务工作目录)
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).map_err(ApiError::from)?;
    // V8 CTX-001 R3：workspace_task_id 续作既有工作区（create_for 幂等）；缺省新建本任务。
    let workdir = match workspace_task_id {
        Some(prev) => st.workspaces.create_for(prev).map_err(ApiError::from)?,
        None => st.workspaces.create_for(task.id.as_ref()).map_err(ApiError::from)?,
    };
    let workdir_for_scan = workdir.clone();
    router
        .register(Box::new(forge_exec::WriteFileTool::new(workdir)))
        .map_err(ApiError::from)?;
    // V8 CTX-001：工作区感知工具（read_file / list_dir，root=续作工作区）。
    router
        .register(Box::new(forge_exec::ReadFileTool::new(workdir_for_scan.clone())))
        .map_err(ApiError::from)?;
    router
        .register(Box::new(forge_exec::ListDirTool::new(workdir_for_scan.clone())))
        .map_err(ApiError::from)?;
    // V8 EDIT-001：增量编辑工具（edit_patch，root=续作工作区）。
    router
        .register(Box::new(forge_exec::EditPatchTool::new(workdir_for_scan.clone())))
        .map_err(ApiError::from)?;

    // ORCH-101c：MCP 工具源（未配置零开销；失败仅 warn 不阻断编排）
    let mcp_configs = mcp_tools::configs_from_env();
    if !mcp_configs.is_empty() {
        match mcp_tools::allowlist_from_env() {
            Some(wl) => {
                if let Err(e) = mcp_tools::register_mcp_tools(&router, &mcp_configs, &wl).await {
                    eprintln!("orchestrate: MCP tools registration failed (non-blocking): {e}");
                }
            }
            None => {
                eprintln!("orchestrate: FORGE_MCP_SERVERS set but FORGE_MCP_ALLOWLIST empty — MCP tools skipped (whitelist required)");
            }
        }
    }

    // 规划器：配置了大模型且开启 codegen/multistep 时
    // 用真实模型做多步规划/单文件生成；未配置则回退确定性
    // SequentialPlanner（离线全绿的基线语义）。
    let llm_cfg = st.llm_config.read().await.clone();
    let llm_ready = !llm_cfg.base_url.trim().is_empty() && !llm_cfg.api_key.trim().is_empty();
    // BILL-001：LLM token 计量钩子（R6-026，构造期注入）
    let meter = st
        .pool
        .clone()
        .map(|pool| {
            Arc::new(billing::PgLlmMeter { pool, tenant: tenant.to_string() })
                as Arc<dyn forge_plan_llm::usage::LlmMeter>
        });
    // ORCH-101a：Codegen 快速路径保留；MultiStep 接通 LlmPlanner + LlmReplanner 回路
    let (planner, replanner): (PlannerOpt, ReplannerOpt) = match plan_mode {
            PlanMode::Codegen => {
                let p = if llm_ready { Some(build_codegen_planner(&llm_cfg, meter)) } else { None };
                (p, None)
            }
            PlanMode::MultiStep => {
                if llm_ready {
                    match build_multistep_planner(
                        &llm_cfg,
                        meter.clone(),
                        Some(build_workspace_context(&workdir_for_scan)),
                    ) {
                        Ok((p, rp)) => (p, rp),
                        Err(e) => {
                            eprintln!("orchestrate: multistep planner unavailable ({e}), falling back to sequential");
                            (None, None)
                        }
                    }
                } else {
                    eprintln!("orchestrate: MultiStep without LLM config, falling back to sequential planner");
                    (None, None)
                }
            }
            PlanMode::Auto => unreachable!("resolve_plan_mode 不返回 Auto"),
        };

    // ORCH-101b：仅 MultiStep 走沙箱化命令验收（基线路径零回归）
    let (verifier_cmd, _uses_sandbox) = sandbox_verify::select_command_verifier(
        plan_mode,
        Arc::new(CommandVerifier),
    );
    let deps = forge_sdk::OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(DemoAllowAll),
        verifier_cmd,
        verifier_file: Arc::new(FileVerifier),
        evidence: st.evidence.clone(),
        workspace: st.workspaces.clone(),
        timeout: Duration::from_secs(timeout_secs),
        // ORCH-003：服务端默认有界重试 + 无 LLM 重规划器（V3.2 流水线再接入真实重规划）
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 1,
            base_backoff_ms: 200,
        }),
        replanner,
        max_replans: 1,
        planner,
        workspace_task: workspace_task_id.map(|s| s.to_string()),
    };
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(timeout_secs) };
    let report = match st.sdk.run_end_to_end(&task.id, &deps, &orch).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("orchestrate: execution failed for task {}: {e}", task.id);
            let _ = st.sdk.tasks().update_status(&task.id, TaskStatus::Failed).await;
            return Err(ApiError::from(e));
        }
    };

    // BILL-001：三维度计量（R4 失败只 warn 不阻断）
    if let Some(pool) = st.pool.clone() {
        billing::record_usage(&pool, tenant, "task_count", 1,
            serde_json::json!({ "task_id": task.id.to_string() })).await;
        let bytes = billing::workspace_bytes(&workdir_for_scan).await;
        billing::record_usage(&pool, tenant, "storage_bytes", bytes as i64,
            serde_json::json!({ "task_id": task.id.to_string() })).await;
    }

    record_report(st, &report).await;
    Ok(report)
}

/// OBS-002 计数 + GA-FIX-2 失败自动入知识库（直连路径与队列 worker 共用）。
async fn record_report(st: &AppState, report: &forge_sdk::OrchestratorReport) {
    use std::sync::atomic::Ordering;
    st.metrics.executions_total.fetch_add(1, Ordering::Relaxed);
    for v in &report.verifications {
        if matches!(v.verdict, forge_verify::Verdict::Pass) {
            st.metrics.verifications_pass.fetch_add(1, Ordering::Relaxed);
        } else {
            st.metrics.verifications_fail.fetch_add(1, Ordering::Relaxed);
        }
    }
    st.metrics
        .replans_total
        .fetch_add(u64::from(report.replans_used), Ordering::Relaxed);

    if report.final_status == TaskStatus::Failed {
        let summary = report
            .execution
            .failed
            .as_ref()
            .map(|(s, r)| format!("step {s}: {r}"))
            .unwrap_or_else(|| format!("gate_failed={}", !report.gate.passed));
        let summary: String = summary.chars().take(200).collect();
        if let Ok(record) = forge_recovery::classify(
            &forge_core::new_execution_id(),
            forge_exec::ExecutionStatus::Failed,
            &summary,
        ) {
            st.knowledge
                .ingest(KnowledgeEntry {
                    record,
                    related_evidence: report.evidence_ids.clone(),
                    tool: Some("orchestrate".into()),
                })
                .await;
        }
    }
}

/// 编排报告 → HTTP 响应体（直连与队列自认领共用，字段形状冻结）。
fn report_response(report: &forge_sdk::OrchestratorReport) -> Json<serde_json::Value> {
    // V7：带验收明细（verdict+reason），排障与前端直读
    let verifications: Vec<serde_json::Value> = report
        .verifications
        .iter()
        .map(|v| {
            serde_json::json!({
                "criterion_id": v.criterion_id,
                "verdict": format!("{:?}", v.verdict),
                "reason": v.reason,
            })
        })
        .collect();
    Json(serde_json::json!({
        "task_id": report.task_id.to_string(),
        "final_status": format!("{:?}", report.final_status),
        "gate_passed": report.gate.passed,
        "steps_completed": report.execution.completed.len(),
        "evidence_count": report.evidence_ids.len(),
        "replans_used": report.replans_used,
        "escalated_to_human": report.escalated_to_human,
        "plan_versions": report.plan_versions.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "verifications": verifications,
        "evidence_ids": report.evidence_ids.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
    }))
}

async fn orchestrate(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Json(req): Json<OrchestrateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // TEN-003（V5-FIX-2b）：身份解析后、执行前检查配额
    let tenant = match outcome.0 {
        auth::AuthOutcome::Tenant(t) => t,
        auth::AuthOutcome::Local => auth::DEFAULT_TENANT.to_string(),
    };
    let qv = st.quotas.of(&tenant).await.map_err(ApiError::from)?;
    let running = st.sdk.tasks().count_running(&tenant).await.map_err(ApiError::from)?;
    let today_count = st.sdk.tasks().count_today(&tenant).await.map_err(ApiError::from)?;
    quota::check_quota(&qv, running, today_count).await.map_err(ApiError::from)?;

    let task = st.sdk.create_task(req.goal.clone(), vec![], req.acceptance.clone()).await?;
    let plan_mode = resolve_plan_mode(req.plan_mode, req.codegen_flag);

    // FED-001 R4：PG 模式且未设 FORGE_QUEUE_INLINE=1 时走"入队 → 认领循环执行"。
    // 单副本行为等价：本 handler 自认领自己的任务并同步返回完整报告；
    // 多副本下自己的任务可能被其它副本 worker 抢走 → 轮询任务状态至终态，
    // 返回降级响应（final_status 可得，报告明细留在执行副本）。
    let inline = std::env::var("FORGE_QUEUE_INLINE").ok().as_deref() == Some("1");
    if let Some(pool) = st.pool.clone() {
        if !inline {
            let payload = serde_json::json!({
                "timeout_secs": req.timeout_secs,
                "codegen_flag": req.codegen_flag,
                // R1：经 serde 序列化（snake_case 冻结）；Debug+lowercase 曾产生
                // "multistep" 与解析端 "multi_step" 失配——真实 E2E 撞出（V7 报告）
                "plan_mode": plan_mode,
                "workspace_task_id": req.workspace_task_id,
            });
            queue::enqueue_orchestration(&pool, task.id.as_ref(), &tenant, payload)
                .await
                .map_err(ApiError::from)?;

            let worker = queue::new_worker_id();
            loop {
                let claimed = queue::claim_next_task(&pool, &worker, 300)
                    .await
                    .map_err(ApiError::from)?;
                match claimed {
                    Some(qt) => {
                        let ours = qt.task_id == task.id.as_ref();
                        let result = execute_queued(&st, &qt, &tenant).await;
                        let _ = queue::complete_task(&pool, qt.id, result.is_ok()).await;
                        if ours {
                            let report = result?;
                            return Ok(report_response(&report));
                        }
                        // 认领到了其它副本入队的任务：执行完继续找自己的
                    }
                    None => {
                        // 无可认领 → 自己的任务在其它副本手里，轮询至终态
                        return Ok(poll_terminal_response(&st, &task, req.timeout_secs).await);
                    }
                }
            }
        }
    }

    let report = execute_orchestration(
        &st, &task, req.timeout_secs, plan_mode, &tenant, req.workspace_task_id.as_deref(),
    ).await?;
    Ok(report_response(&report))
}

/// 队列 worker 执行体：按 payload 取任务并跑完整编排。
async fn execute_queued(st: &AppState, qt: &queue::QueuedTask, tenant: &str) -> Result<forge_sdk::OrchestratorReport, ApiError> {
    let task = st
        .sdk
        .get_task(&TaskId::from(qt.task_id.clone()))
        .await
        .map_err(ApiError::from)?;
    let timeout_secs = qt.payload["timeout_secs"].as_u64().unwrap_or(30);
    let plan_mode: PlanMode = qt
        .payload
        .get("plan_mode")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| resolve_plan_mode(None, qt.payload["codegen_flag"].as_bool().unwrap_or(true)));
    let workspace_task_id = qt.payload["workspace_task_id"].as_str();
    execute_orchestration(st, &task, timeout_secs, plan_mode, tenant, workspace_task_id).await
}

/// 降级响应：任务被其它副本 worker 认领，轮询本库任务状态至终态。
async fn poll_terminal_response(
    st: &AppState,
    task: &Task,
    timeout_secs: u64,
) -> Json<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs + 30);
    let mut final_status = String::from("Pending");
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(t) = st.sdk.get_task(&task.id).await {
            final_status = format!("{:?}", t.status);
            if matches!(t.status, TaskStatus::Completed | TaskStatus::Failed) {
                break;
            }
        }
    }
    Json(serde_json::json!({
        "task_id": task.id.to_string(),
        "final_status": final_status,
        "queued": true,
    }))
}

/// FED-001：后台认领循环（FORGE_QUEUE_WORKER=1 时启用，多副本部署口径）。
async fn worker_loop(st: AppState, worker_id: String) {
    let Some(pool) = st.pool.clone() else { return };
    match queue::reap_expired_leases(&pool).await {
        Ok(n) if n > 0 => eprintln!("queue worker: reaped {n} expired leases"),
        Err(e) => eprintln!("queue worker: reap failed: {e}"),
        _ => {}
    }
    loop {
        match queue::claim_next_task(&pool, &worker_id, 300).await {
            Ok(Some(qt)) => {
                let ok = execute_queued(&st, &qt, &qt.tenant_id).await.is_ok();
                if let Err(e) = queue::complete_task(&pool, qt.id, ok).await {
                    eprintln!("queue worker: complete failed: {e}");
                }
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(e) => {
                eprintln!("queue worker: claim failed: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn put_evidence(
    State(st): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use forge_evidence::{Evidence, EvidenceKind};
    let ev = Evidence {
        id: forge_core::new_evidence_id(),
        kind: EvidenceKind::Log,
        criterion_id: body["criterion_id"].as_str().unwrap_or("").into(),
        content: body["content"].as_str().unwrap_or("").into(),
        produced_by: body["produced_by"].as_str().unwrap_or("manual").into(),
        at: chrono::Utc::now(),
    };
    let id = st.evidence.put(ev).await.map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({"evidence_id": id.to_string()})))
}

async fn get_evidence(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let eid = forge_core::EvidenceId::from(id);
    let ev = st.evidence.get(&eid).await.map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "id": ev.id.to_string(),"kind": format!("{:?}", ev.kind),
        "criterion_id": ev.criterion_id,"content": ev.content,
        "produced_by": ev.produced_by,"at": ev.at.to_rfc3339(),
    })))
}

// ==================== V4.0 产品工厂（PROD-001/002） ====================

#[derive(Deserialize)]
pub struct PublishTemplateRequest {
    pub template: forge_product::ProductTemplate,
    pub version: String,
    /// Reviewer 裁决（V3.2 衔接）：仅接受 "Pass" | "Concern"。
    pub review_verdict: String,
}

async fn publish_template(
    State(st): State<AppState>,
    Json(req): Json<PublishTemplateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = format!("{}@{}", req.template.id, req.version);
    let rec = forge_product_instance::TemplateRecord {
        template: req.template,
        version: req.version,
        review_verdict: req.review_verdict,
        published_at: chrono::Utc::now(),
    };
    st.templates.publish(rec).await.map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "published": id })))
}

async fn list_templates(State(st): State<AppState>) -> Json<serde_json::Value> {
    let list = st.templates.list().await.unwrap_or_default();
    let items: Vec<_> = list
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.template.id,
                "version": r.version,
                "name": r.template.name,
                "review_verdict": r.review_verdict,
            })
        })
        .collect();
    Json(serde_json::json!({ "count": items.len(), "templates": items }))
}

#[derive(Deserialize)]
pub struct InstantiateRequest {
    pub template_id: String,
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub params: std::collections::HashMap<String, String>,
}

async fn instantiate_product(
    State(st): State<AppState>,
    Json(req): Json<InstantiateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use forge_product_instance::ProductState;
    let rec = st.templates.get(&req.template_id, &req.version).await.map_err(ApiError::from)?;
    let manifest = forge_product::instantiate(&rec.template, &req.params).map_err(ApiError::from)?;
    manifest.validate().map_err(ApiError::from)?;

    let now = chrono::Utc::now();
    let inst = forge_product_instance::ProductInstance {
        id: forge_product_instance::new_product_instance_id(),
        template_id: req.template_id,
        template_version: req.version,
        name: req.name.unwrap_or_else(|| format!("inst-{}", manifest.name)),
        params: req.params,
        state: ProductState::Draft,
        created_at: now,
        updated_at: now,
    };
    let iid = inst.id.clone();
    st.instances.insert(inst).await.map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "instance_id": iid, "state": "Draft" })))
}

async fn list_products(State(st): State<AppState>) -> Json<serde_json::Value> {
    let list = st.instances.list().await.unwrap_or_default();
    let items: Vec<_> = list
        .into_iter()
        .map(|i| {
            serde_json::json!({
                "id": i.id, "name": i.name, "state": format!("{:?}", i.state),
                "template_id": i.template_id, "template_version": i.template_version,
            })
        })
        .collect();
    Json(serde_json::json!({ "count": items.len(), "products": items }))
}

async fn get_product(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<forge_product_instance::ProductInstance>, ApiError> {
    Ok(Json(st.instances.get(&id).await.map_err(ApiError::from)?))
}

async fn transition_product(
    State(st): State<AppState>,
    Path(id): Path<String>,
    to: forge_product_instance::ProductState,
) -> Result<Json<forge_product_instance::ProductInstance>, ApiError> {
    let mut inst = st.instances.get(&id).await.map_err(ApiError::from)?;
    inst.transition(to).map_err(ApiError::from)?;
    st.instances.update(inst.clone()).await.map_err(ApiError::from)?;
    Ok(Json(inst))
}

async fn product_start(
    st: State<AppState>,
    path: Path<String>,
) -> Result<Json<forge_product_instance::ProductInstance>, ApiError> {
    transition_product(st, path, forge_product_instance::ProductState::Active).await
}

async fn product_stop(
    st: State<AppState>,
    path: Path<String>,
) -> Result<Json<forge_product_instance::ProductInstance>, ApiError> {
    transition_product(st, path, forge_product_instance::ProductState::Stopped).await
}

async fn product_deprecate(
    st: State<AppState>,
    path: Path<String>,
) -> Result<Json<forge_product_instance::ProductInstance>, ApiError> {
    transition_product(st, path, forge_product_instance::ProductState::Deprecated).await
}

// ==================== OBS-002 metrics ====================

async fn metrics_handler(State(st): State<AppState>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        st.metrics.render(),
    )
}


// ==================== KNW-001 服务面（GA-FIX-2） ====================

#[derive(Deserialize)]
pub struct KnowledgeFailuresQuery {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tool_like: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

async fn knowledge_failures_handler(
    State(st): State<AppState>,
    Query(q): Query<KnowledgeFailuresQuery>,
) -> Result<Json<Vec<KnowledgeEntry>>, ApiError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as usize;
    let category = match q.category.as_deref() {
        Some("ToolError") => Some(FailureCategory::ToolError),
        Some("Timeout") => Some(FailureCategory::Timeout),
        Some("PermissionDenied") => Some(FailureCategory::PermissionDenied),
        Some(_) | None => None,
    };
    let tool = q.tool_like.as_deref();
    let entries = st.knowledge.search(category, tool, None).await;
    Ok(Json(entries.into_iter().take(limit).collect()))
}

async fn knowledge_export_handler(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ReplayArchive>, ApiError> {
    let sid = forge_core::SessionId::from(id);
    let archive = forge_knowledge::export_replay(st.sdk.sessions(), &sid)
        .await
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(archive))
}

// ==================== BILL-001 计量查询面 ====================

#[derive(Deserialize)]
pub struct UsageQuery {
    pub tenant: Option<String>,
    /// RFC3339，缺省 = 近 30 天
    pub from: Option<String>,
    pub to: Option<String>,
}

async fn admin_usage(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = match outcome.0 {
        auth::AuthOutcome::Tenant(t) => t,
        auth::AuthOutcome::Local => auth::DEFAULT_TENANT.to_string(),
    };
    let tenant = q.tenant.clone().unwrap_or_else(|| caller.clone());
    // 门禁：本租户或 DEFAULT_TENANT 管理员
    if caller != tenant && caller != auth::DEFAULT_TENANT {
        return Err(ApiError(StatusCode::FORBIDDEN, "usage query requires default tenant admin".into()));
    }
    let Some(pool) = st.pool.clone() else {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "usage requires PostgreSQL storage".into()));
    };
    let parse = |s: &Option<String>, default| -> Result<chrono::DateTime<chrono::Utc>, ApiError> {
        match s {
            Some(v) => chrono::DateTime::parse_from_rfc3339(v)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("bad rfc3339: {e}"))),
            None => Ok(default),
        }
    };
    let to = parse(&q.to, chrono::Utc::now())?;
    let from = parse(&q.from, to - chrono::Duration::days(30))?;
    let rows = billing::summarize_usage(&pool, &tenant, from, to).await.map_err(ApiError::from)?;
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, sum)| serde_json::json!({ "kind": kind, "sum": sum }))
        .collect();
    Ok(Json(serde_json::json!({ "tenant": tenant, "from": from.to_rfc3339(), "to": to.to_rfc3339(), "items": items })))
}

// ==================== BILL-002 管理面（费率与账单，仅 DEFAULT_TENANT） ====================

/// admin 门禁：Local 视为 default 管理员；非 default 租户 → 403。
fn require_default_admin(outcome: &auth::AuthOutcome) -> Result<String, (StatusCode, String)> {
    let tenant = match outcome {
        auth::AuthOutcome::Tenant(t) => t.clone(),
        auth::AuthOutcome::Local => auth::DEFAULT_TENANT.to_string(),
    };
    if tenant != auth::DEFAULT_TENANT {
        return Err((StatusCode::FORBIDDEN, "admin requires default tenant".into()));
    }
    Ok(tenant)
}

#[derive(Deserialize)]
pub struct SetRateRequest {
    pub tenant_id: String,
    pub kind: String,
    pub unit_price_micros: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
}
fn default_currency() -> String { "CNY".into() }

async fn admin_set_rate(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Json(req): Json<SetRateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_default_admin(&outcome.0)?;
    let Some(pool) = st.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "billing requires PostgreSQL storage".into()));
    };
    billing::set_rate(&pool, &req.tenant_id, &req.kind, req.unit_price_micros, &req.currency)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "tenant_id": req.tenant_id, "kind": req.kind,
                "unit_price_micros": req.unit_price_micros, "currency": req.currency })))
}

async fn admin_list_rates(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let caller = require_default_admin(&outcome.0)?;
    let tenant = q.get("tenant_id").cloned().unwrap_or(caller);
    let Some(pool) = st.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "billing requires PostgreSQL storage".into()));
    };
    let rows = billing::list_rates(&pool, &tenant)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(kind, price, currency)| serde_json::json!({"kind": kind, "unit_price_micros": price, "currency": currency}))
        .collect();
    Ok(Json(serde_json::json!({ "tenant_id": tenant, "items": items })))
}

#[derive(Deserialize)]
pub struct GenerateBillRequest {
    pub tenant_id: String,
    pub period_from: String,
    pub period_to: String,
}

async fn admin_generate_bill(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Json(req): Json<GenerateBillRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_default_admin(&outcome.0)?;
    let Some(pool) = st.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "billing requires PostgreSQL storage".into()));
    };
    let parse = |v: &str| chrono::DateTime::parse_from_rfc3339(v)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad rfc3339: {e}")));
    let from = parse(&req.period_from)?;
    let to = parse(&req.period_to)?;
    let (bill_id, doc_hash, doc) = billing::generate_bill(&pool, &req.tenant_id, from, to)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "bill_id": bill_id, "doc_hash": doc_hash, "doc": doc })))
}

async fn admin_get_bill(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_default_admin(&outcome.0)?;
    let Some(pool) = st.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "billing requires PostgreSQL storage".into()));
    };
    let doc = billing::get_bill(&pool, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "bill not found".into()))?;
    Ok(Json(serde_json::json!({ "bill_id": id, "doc": doc })))
}

async fn admin_export_bill(
    State(st): State<AppState>,
    outcome: axum::Extension<auth::AuthOutcome>,
    Path(id): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    require_default_admin(&outcome.0)?;
    let Some(pool) = st.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "billing requires PostgreSQL storage".into()));
    };
    let doc = billing::get_bill(&pool, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "bill not found".into()))?;
    let fmt = q.get("format").map(|s| s.as_str()).unwrap_or("json");
    match fmt {
        "csv" => Ok((
            [(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")],
            billing::bill_to_csv(&doc),
        )
            .into_response()),
        _ => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            serde_json::to_string(&doc).unwrap_or_default(),
        )
            .into_response()),
    }
}

// ==================== UI-001 Web 控制台（纯静态零构建） ====================

async fn ui_index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}
async fn ui_sessions() -> Html<&'static str> {
    Html(include_str!("../static/sessions.html"))
}
async fn ui_evidence() -> Html<&'static str> {
    Html(include_str!("../static/evidence.html"))
}

/// 构建服务端 LLM 规划器（支持运行时动态配置）。
fn llm_model(cfg: &routes::llm::LlmRuntimeConfig) -> String {
    if !cfg.model.trim().is_empty() {
        cfg.model.clone()
    } else {
        "deepseek-chat".into()
    }
}

fn build_llm_backend(cfg: &routes::llm::LlmRuntimeConfig) -> Arc<dyn forge_plan_llm::LlmPlanBackend> {
    Arc::new(forge_api::LlmClient::new(cfg.base_url.clone(), cfg.api_key.clone()))
}

/// Codegen 快速路径（V5.1 契约保留）：单文件代码生成规划器。
fn build_codegen_planner(
    cfg: &routes::llm::LlmRuntimeConfig,
    meter: Option<Arc<dyn forge_plan_llm::usage::LlmMeter>>,
) -> Arc<dyn forge_planner::Planner> {
    let mut planner = forge_plan_llm::SingleFileCodegenPlanner::new(build_llm_backend(cfg), llm_model(cfg));
    planner.meter = meter;
    Arc::new(planner)
}

/// V8 CTX-001 R2：组装工作区上下文注入块（清单行 + ≤32KB 小文件内容，≤8KB/个）。
/// 冻结格式：完整注入块含首行 "=== Workspace context ==="，由 LlmPlanner 直接追加。
pub const CONTEXT_MAX_BYTES: usize = 32 * 1024;
pub const SMALL_FILE_MAX_BYTES: usize = 8 * 1024;

pub fn build_workspace_context(root: &std::path::Path) -> String {
    let mut out = String::from("=== Workspace context ===\n");
    // 清单行：单层列举，按名排序（目录加 '/' 后缀）。
    let mut entries: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let name = e.file_name().to_string_lossy().to_string();
            entries.push(if e.path().is_dir() { format!("{name}/") } else { name });
        }
    }
    for name in &entries {
        out.push_str(name);
        out.push('\n');
    }
    // 小文件内容：≤8KB/个、总量 ≤32KB，超限截断标 (truncated)。
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            if e.path().is_dir() {
                continue;
            }
            if let Ok(meta) = e.metadata() {
                if meta.len() > SMALL_FILE_MAX_BYTES as u64 {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(e.path()) {
                if out.len() + content.len() + 1 > CONTEXT_MAX_BYTES {
                    out.push_str("(truncated)");
                    break;
                }
                out.push_str("\n--- ");
                out.push_str(&e.file_name().to_string_lossy());
                out.push_str(" ---\n");
                out.push_str(&content);
            }
        }
    }
    out
}

/// V7 ORCH-101a：MultiStep 多步规划器 + 重规划器（共享 backend；tools 白名单冻结）。
fn build_multistep_planner(
    cfg: &routes::llm::LlmRuntimeConfig,
    meter: Option<Arc<dyn forge_plan_llm::usage::LlmMeter>>,
    context: Option<String>,
) -> Result<(PlannerOpt, ReplannerOpt), String> {
    let backend = build_llm_backend(cfg);
    // V8 CTX-001 R4：白名单扩为 5 工具（edit_patch 由 EDIT-001 启用）。
    let tools = vec![
        "echo".to_string(),
        "write_file".to_string(),
        "read_file".to_string(),
        "list_dir".to_string(),
        "edit_patch".to_string(),
    ];
    let planner = forge_plan_llm::LlmPlanner {
        backend: backend.clone(),
        model: llm_model(cfg),
        schema_max_attempts: 3,
        tools: tools.clone(),
        ledger: None,
        meter: meter.clone(), // BILL-003：plan 用途入账
        brief_mode: false,
        context: context.clone(),
    };
    let replanner = forge_plan_llm::LlmReplanner {
        backend,
        model: llm_model(cfg),
        schema_max_attempts: 3,
        tools,
        ledger: None,
        meter, // BILL-003：replan 用途入账
    };
    Ok((
        Some(Arc::new(planner) as Arc<dyn forge_planner::Planner>),
        Some(Arc::new(replanner) as Arc<dyn forge_plan_llm::Replanner>),
    ))
}
/// GET /events/stream — SSE 实时事件流（API-003；FED-002 跨副本合并）。
async fn events_stream(
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    use futures::stream;

    // 本地事件 → JSON 字符串 broadcast（merge_event_streams 契约输入形状）
    let es = st.event_bus.subscribe(Topic::Session).await.unwrap();
    let (lbtx, lbrx) = tokio::sync::broadcast::channel::<String>(64);
    tokio::spawn(async move {
        let mut es = es;
        while let Ok(event) = es.recv().await {
            let data = serde_json::to_string(
                &serde_json::json!({"id": event.id, "at": event.at.to_rfc3339()}),
            ).unwrap_or_default();
            let _ = lbtx.send(data);
        }
    });

    // 转发流：PG 模式下监听 forge_events 频道（断连只失去转发流，本地不受影响，R3）
    let relayed = match st.pool.clone() {
        Some(pool) => bus::PgBus::listen(&pool, queue::FORGE_EVENTS_CHANNEL).await.ok(),
        None => None,
    };
    let merged = match relayed {
        Some(rx) => sse_relay::merge_event_streams(lbrx, rx),
        None => {
            let (_dtx, drx) = tokio::sync::mpsc::channel::<String>(1);
            sse_relay::merge_event_streams(lbrx, drx)
        }
    };

    let stream = stream::unfold(merged, |mut rx| async move {
        rx.recv().await.map(|data| {
            (
                Ok(SseEvent::default().event("forge_event").data(data)),
                rx,
            )
        })
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

// ==================== CORS（API-004 + SEC-001 默认关闭） ====================

/// CORS 层：默认（FORGE_CORS_ORIGINS 未设置/为空）**关闭**——不带任何 CORS 头；
/// 配置白名单后才允许对应来源。生产基线：不暴露即最安全。
fn maybe_cors() -> Option<tower_http::cors::CorsLayer> {
    let origins = std::env::var("FORGE_CORS_ORIGINS").unwrap_or_default();
    if origins.trim().is_empty() {
        return None;
    }
    let allow: Vec<_> = origins
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    Some(tower_http::cors::CorsLayer::new().allow_origin(allow))
}

// ==================== 路由 ====================

pub fn app() -> Router { app_with_state(AppState::in_memory()) }

pub fn app_with_state(st: AppState) -> Router {
    let router = Router::new()
        .route("/health", get(health))
        .route("/tasks", post(create_task).get(list_tasks))
        .route("/tasks/:id", get(get_task))
        .route("/sessions/:id", get(get_session))
        .route("/orchestrate", post(orchestrate))
        .route("/api/evidence", post(put_evidence))
        .route("/api/evidence/:id", get(get_evidence))
        .route("/events/stream", get(events_stream))
        // V4.0 产品工厂
        .route("/templates", post(publish_template).get(list_templates))
        .route("/products/instantiate", post(instantiate_product))
        .route("/products", get(list_products))
        .route("/products/:id", get(get_product))
        .route("/products/:id/start", post(product_start))
        .route("/products/:id/stop", post(product_stop))
        .route("/products/:id/deprecate", post(product_deprecate))
        // V4.0 观测 + 控制台
        .route("/metrics", get(metrics_handler))
        // KNW-001 服务面
        .route("/knowledge/failures", get(knowledge_failures_handler))
        .route("/knowledge/sessions/:id/export", get(knowledge_export_handler))
        // V5.0 MKT-001/002 市场目录
        .route("/market/capabilities", get(routes::market::list_capabilities))
            .route("/market/templates", get(routes::market::list_market_templates))
            .route("/market/install", post(routes::market::install_capability))
        .route("/market/publish", post(routes::market::publish_release))
        .route("/market/review", post(routes::market::review_release))
        .route("/market/releases", get(routes::market::list_releases))
        .route("/admin/usage", get(admin_usage))
        .route("/admin/rates", post(admin_set_rate).get(admin_list_rates))
        .route("/admin/bills/generate", post(admin_generate_bill))
        .route("/admin/bills/:id", get(admin_get_bill))
        .route("/admin/bills/:id/export", get(admin_export_bill))
        // LLM 动态配置与连通测试
        .route("/api/llm/config", get(routes::llm::get_llm_config).post(routes::llm::update_llm_config))
        .route("/api/llm/test", post(routes::llm::test_llm_connection))
        .route("/", get(ui_index))
        .route("/ui/sessions", get(ui_sessions))
        .route("/ui/evidence", get(ui_evidence));

    // SEC-001 + V5-FIX-2a：鉴权中间件接线（AuthConfig/TenantKeyStore 经 AppState 注入；
    // /health 永远放行，其余路由在启用鉴权时要求 Bearer；解析结果 AuthOutcome
    // 写入 extensions 供下游租户过滤/配额取用）
    let router = router.layer(axum::middleware::from_fn_with_state(
        st.clone(),
        auth::auth_middleware,
    ));

    // SEC-001：CORS 默认关闭，白名单显式配置后才挂层
    let router = if let Some(cors) = maybe_cors() {
        router.layer(cors)
    } else {
        router
    };

    router.with_state(st)
}

// ==================== 启动 ====================

pub async fn run_from_env() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init()
        .ok();
    let port: u16 = std::env::var("FORGE_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8080);
    let host = std::env::var("FORGE_HOST").unwrap_or_else(|_| "127.0.0.1".into());

    // SEC-001：非 loopback 监听必须配置 FORGE_API_KEY，否则拒绝启动（而非告警）
    security_gate(&host, std::env::var("FORGE_API_KEY").ok().as_deref())
        .map_err(std::convert::Into::<Box<dyn std::error::Error>>::into)?;

        let state = match std::env::var("FORGE_PG_URL") {
            Ok(url) => {
                println!("storage: PostgreSQL ({url})");
                // FED-001：显式建池（含 DEP-001 env 参数化），AppState 持有以支撑队列路径
                let pool = forge_storage::connect_and_migrate(&url).await?;
                // FED-002：会话事件追加后转发到跨副本总线（失败不阻断写入）
                let sessions: Arc<dyn SessionStore> = Arc::new(
                    sse_relay::RelaySessionStore::new(
                        Arc::new(forge_storage::PgSessionStore::new(pool.clone())),
                        Arc::new(sse_relay::PgRelay::new(pool.clone())),
                    ),
                );
                AppState {
                    sdk: ForgeSdk::from_stores(
                        Arc::new(forge_storage::PgTaskStore::new(pool.clone())),
                        sessions,
                    ),
                    evidence: Arc::new(InMemoryEvidenceStore::default()),
                    workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
                    event_bus: Arc::new(InMemoryEventBus::with_buffer(sse_buffer())),
                    instances: Arc::new(Default::default()),
                    templates: Arc::new(Default::default()),
                    metrics: Arc::new(Metrics::default()),
                    knowledge: Arc::new(Default::default()),
                    capabilities: Arc::new(Default::default()),
                    auth: AuthConfig::from_env(),
                    // TEN-004 R1：PG 模式用 PG 实现（重启不丢）；内存模式保留内存实现
                    tenant_keys: Arc::new(auth::PgTenantKeyStore::new(pool.clone())),
                    quotas: Arc::new(quota::PgQuotaStore::new(pool.clone())),
                    pool: Some(pool),
                    llm_config: Arc::new(tokio::sync::RwLock::new(routes::llm::LlmRuntimeConfig::from_env())),
                }
            }
            Err(_) => { println!("storage: in-memory"); AppState::in_memory() }
        };
        // FED-001：多副本部署口径下显式拉起后台认领 worker（FORGE_QUEUE_WORKER=1）
        if state.pool.is_some() && std::env::var("FORGE_QUEUE_WORKER").ok().as_deref() == Some("1") {
            let wid = queue::new_worker_id();
            println!("queue worker started ({wid})");
            tokio::spawn(worker_loop(state.clone(), wid));
        }
    let app = app_with_state(state);
    let addr = format!("{host}:{port}")
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("invalid FORGE_HOST '{host}': {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    println!("forge-server listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}

/// SEC-001 启动门禁：非 loopback 监听时必须配置非空 `FORGE_API_KEY`。
///
/// 返回 Err(原因) 表示拒绝启动。loopback（127.x/::1/localhost）不受限，
/// 保持"本地模式零配置可跑"的开发体验（R-03 缓解与 R-03 的生产面收紧并存）。
pub fn security_gate(host: &str, api_key: Option<&str>) -> Result<(), String> {
    // SEC-001 配置性逃生门：仅限本机调试，生产严禁设置
    if std::env::var("FORGE_INSECURE_LOCAL").ok().as_deref() == Some("1") {
        eprintln!("[WARN] FORGE_INSECURE_LOCAL=1 — SEC-001 检查已跳过，仅限本机调试");
        return Ok(());
    }
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if loopback {
        return Ok(());
    }
    let has_key = api_key.map(|k| !k.trim().is_empty()).unwrap_or(false);
    if !has_key {
        return Err(format!(
            "SEC-001: refusing to listen on non-loopback '{host}' without FORGE_API_KEY. \
Set FORGE_API_KEY or bind 127.0.0.1/localhost."
        ));
    }
    Ok(())
}

/// SEC-001 配置性拒绝判定：供 main 选择退出码（78 = EX_CONFIG 惯例）。
///
/// 机械适配说明：规格模板用 `anyhow::Error`，本项目无 anyhow 依赖，
/// 等价改为 `&dyn std::fmt::Display`（run_from_env 的错误链为
/// `Box<dyn Error>`，其 Display 保留 "SEC-001:" 前缀，判定语义一致）。
pub fn is_config_rejection(err: &dyn std::fmt::Display) -> bool {
    err.to_string().starts_with("SEC-001:")
}
