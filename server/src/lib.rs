//! forge-server：Aion Forge 2.0 HTTP API 层（V3.0 完整实现 + V4.0 产品工厂）。
//!
//! 端点：health / tasks CRUD / sessions / orchestrate / evidence / events/stream
//!       /templates /products（生命周期）/ metrics / 控制台三页
//! 安全基线（SEC-001）：Bearer 鉴权（FORGE_API_KEY）、CORS 默认关闭、
//!       非 loopback 监听未配 key 拒绝启动。

pub mod auth;
pub mod quota;
pub mod routes;

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
use forge_cap::{CapabilityRegistry as _, InMemoryCapabilityRegistry};
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
}

impl AppState {
    pub fn in_memory() -> Self {
        Self {
            sdk: ForgeSdk::in_memory(),
            evidence: Arc::new(InMemoryEvidenceStore::default()),
            workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
            event_bus: Arc::new(InMemoryEventBus::new()),
            instances: Arc::new(Default::default()),
            templates: Arc::new(Default::default()),
            metrics: Arc::new(Metrics::default()),
            knowledge: Arc::new(Default::default()),
            capabilities: Arc::new(Default::default()),
        }
    }
    pub fn new(tasks: Arc<dyn TaskStore>, sessions: Arc<dyn SessionStore>) -> Self {
        Self {
            sdk: ForgeSdk::from_stores(tasks, sessions),
            evidence: Arc::new(InMemoryEvidenceStore::default()),
            workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
            event_bus: Arc::new(InMemoryEventBus::new()),
            instances: Arc::new(Default::default()),
            templates: Arc::new(Default::default()),
            metrics: Arc::new(Metrics::default()),
            knowledge: Arc::new(Default::default()),
        }
    }
}

// ==================== 错误 ====================

pub struct ApiError(StatusCode, String);

impl From<ForgeError> for ApiError {
    fn from(e: ForgeError) -> Self {
        let status = match &e {
            ForgeError::NotFound(_) => StatusCode::NOT_FOUND,
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
    fn into_response(self) -> Response { (self.0, self.1).into_response() }
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
}
fn default_timeout() -> u64 { 30 }

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

async fn list_tasks(State(st): State<AppState>) -> Json<serde_json::Value> {
    let ids = st.sdk.list_tasks().await.unwrap_or_default();
    Json(serde_json::json!({"count": ids.len(), "ids": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>()}))
}

async fn get_task(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<Task>, ApiError> {
    Ok(Json(st.sdk.get_task(&TaskId::from(id)).await?))
}

async fn get_session(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<forge_session::Session>, ApiError> {
    Ok(Json(st.sdk.sessions().get(&SessionId::from(id)).await?))
}

async fn orchestrate(
    State(st): State<AppState>,
    Json(req): Json<OrchestrateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = st.sdk.create_task(req.goal.clone(), vec![], req.acceptance.clone()).await?;

    // 工具集：echo(基线) + write_file("写软件"落盘能力，根=任务工作目录)
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).map_err(ApiError::from)?;
    let workdir = st.workspaces.create_for(task.id.as_ref()).map_err(ApiError::from)?;
    router
        .register(Box::new(forge_exec::WriteFileTool::new(workdir)))
        .map_err(ApiError::from)?;

    // 规划器：配置了 FORGE_LLM_* 时用真实模型做 Architect（工具白名单=write_file）；
    // 未配置则回退确定性 SequentialPlanner（离线全绿的基线语义）。
    let llm_ready = !std::env::var("FORGE_LLM_BASE_URL")
        .unwrap_or_default()
        .trim()
        .is_empty()
        && !std::env::var("FORGE_LLM_API_KEY")
            .unwrap_or_default()
            .trim()
            .is_empty();
    let planner: Option<Arc<dyn forge_planner::Planner>> = if llm_ready {
        match build_llm_planner().await {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("orchestrate: LLM planner unavailable ({e}), falling back to sequential");
                None
            }
        }
    } else {
        None
    };

    let deps = forge_sdk::OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(DemoAllowAll),
        verifier_cmd: Arc::new(CommandVerifier),
        verifier_file: Arc::new(FileVerifier),
        evidence: st.evidence.clone(),
        workspace: st.workspaces.clone(),
        timeout: Duration::from_secs(req.timeout_secs),
        // ORCH-003：服务端默认有界重试 + 无 LLM 重规划器（V3.2 流水线再接入真实重规划）
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 1,
            base_backoff_ms: 200,
        }),
        replanner: None,
        max_replans: 1,
        planner,
    };
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(req.timeout_secs) };
    let report = st.sdk.run_end_to_end(&task.id, &deps, &orch).await.map_err(ApiError::from)?;

    // OBS-002：执行计数与验收结果计数
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

    // GA-FIX-2：失败自动入知识库（KNW-001 服务面闭环）
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

    Ok(Json(serde_json::json!({
        "task_id": report.task_id.to_string(),
        "final_status": format!("{:?}", report.final_status),
        "gate_passed": report.gate.passed,
        "steps_completed": report.execution.completed.len(),
        "evidence_count": report.evidence_ids.len(),
        "replans_used": report.replans_used,
        "escalated_to_human": report.escalated_to_human,
        "plan_versions": report.plan_versions.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
    })))
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

/// 单文件代码生成规划器（"写软件"路径核心，对小上限模型鲁棒）。
///
/// 两次**纯文本**调用，全程无 JSON：
/// 1. 问文件名（短输出）；
/// 2. 要完整文件内容（原始代码，strip 围栏后落盘）。
///
/// 执行写入与运行验收由确定性引擎/Verifier 完成，LLM 只负责产出代码。
struct SingleFileCodegenPlanner {
    backend: Arc<forge_api::LlmClient>,
    model: String,
}

fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let mut lines: Vec<&str> = t.lines().collect();
    if lines.len() >= 2 {
        lines.remove(0); // ```lang
        if lines.last().map(|l| l.trim() == "```").unwrap_or(false) {
            lines.pop();
        }
        lines.join("\n")
    } else {
        t.to_string()
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("output.txt")
        .to_string()
}

#[async_trait::async_trait]
impl forge_planner::Planner for SingleFileCodegenPlanner {
    async fn plan(&self, task: &forge_task::Task) -> ForgeResult<forge_planner::Plan> {
        use forge_api::{ChatMessage, LlmBackend as _};

        // ① 文件名（短输出，天然不受长补全截断影响）
        let name_sys = "Answer with a single output filename including extension (like hello.py or app.js). No explanation.";
        let name_user = format!("Task: {}\nPick the best output filename.", task.goal);
        let name_raw = self
            .backend
            .chat(&self.model, &[ChatMessage::system(name_sys), ChatMessage::user(name_user)])
            .await?;
        let filename = first_line(&strip_code_fence(&name_raw));

        // ② 完整文件内容（纯文本，无 JSON 转义/截断问题）
        let mut ac_text = String::new();
        for a in &task.acceptance {
            ac_text.push_str(&format!("- {}: {}\n", a.id, a.description));
        }
        let code_sys = "You are a senior engineer. Output ONLY the complete final content of the requested file. No markdown fences, no explanations.";
        let code_user = format!(
            "Filename: {filename}\nIt will be verified by:\n{ac_text}\nGoal: {goal_text}\nWrite the complete file now.",
            goal_text = task.goal,
        );
        let raw = self
            .backend
            .chat(&self.model, &[ChatMessage::system(code_sys), ChatMessage::user(code_user)])
            .await?;
        let content = strip_code_fence(&raw);
        eprintln!("codegen[{filename}] bytes={}", content.len());

        Ok(forge_planner::Plan {
            id: forge_core::new_plan_id(),
            task_id: task.id.clone(),
            steps: vec![forge_planner::PlanStep {
                id: "codegen".into(),
                title: format!("生成 {filename}"),
                depends_on: vec![],
                action: forge_planner::StepAction::CallCapability {
                    capability: "write_file".into(),
                    input: serde_json::json!({ "path": filename, "content": content }),
                },
            }],
            status: forge_planner::PlanStatus::Ready,
        })
    }
}
/// 构建服务端 LLM 规划器（单文件代码生成，纯文本双调用，对小上限模型鲁棒）。
async fn build_llm_planner() -> Result<Arc<dyn forge_planner::Planner>, String> {
    let base = std::env::var("FORGE_LLM_BASE_URL").unwrap_or_default();
    let key = std::env::var("FORGE_LLM_API_KEY").unwrap_or_default();
    Ok(Arc::new(SingleFileCodegenPlanner {
        backend: Arc::new(forge_api::LlmClient::new(base, key)),
        model: std::env::var("FORGE_TIER_HIGH_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "glm-5.2".into()),
    }))
}
/// GET /events/stream — SSE 实时事件流（API-003）
async fn events_stream(
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    use futures::stream;

    let es = st.event_bus.subscribe(Topic::Session).await.unwrap();
    let stream = stream::unfold(es, |mut es| async move {
        // unfold 每次调用产出一个事件；通道关闭(Err)时返回 None 结束流。
        match es.recv().await {
            Ok(event) => {
                let data = serde_json::to_string(
                    &serde_json::json!({"id": event.id, "at": event.at.to_rfc3339()}),
                ).unwrap_or_default();
                Some((
                    Ok(SseEvent::default().event("forge_event").data(data)),
                    es,
                ))
            }
            Err(_) => None,
        }
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
        .nest("/market", Router::new()
            .route("/capabilities", get(routes::market::list_capabilities))
            .route("/templates", get(routes::market::list_market_templates))
            .route("/install", post(routes::market::install_capability))
        )
        .route("/", get(ui_index))
        .route("/ui/sessions", get(ui_sessions))
        .route("/ui/evidence", get(ui_evidence));

    // SEC-001：鉴权中间件接线（AuthConfig 经 Extension 注入；
    // /health 永远放行，其余路由在启用 FORGE_API_KEY 时要求 Bearer）
    let router = router
        .layer(axum::middleware::from_fn(auth::auth_middleware))
        .layer(axum::Extension(AuthConfig::from_env()));

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
            AppState {
                sdk: ForgeSdk::postgres(&url).await?,
                evidence: Arc::new(InMemoryEvidenceStore::default()),
                workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
                event_bus: Arc::new(InMemoryEventBus::new()),
                instances: Arc::new(Default::default()),
                templates: Arc::new(Default::default()),
                metrics: Arc::new(Metrics::default()),
                knowledge: Arc::new(Default::default()),
            }
        }
        Err(_) => { println!("storage: in-memory"); AppState::in_memory() }
    };
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
