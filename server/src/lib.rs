//! forge-server：Aion Forge 2.0 HTTP API 层（V3.0 完整实现）。
//!
//! 端点：health / tasks CRUD / sessions / orchestrate / evidence / events/stream
//! 中间件：CORS 白名单（env FORGE_CORS_ORIGINS）

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event as SseEvent, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use forge_core::{ForgeError, ForgeResult, SessionId, TaskId};
use forge_evidence::{EvidenceStore, InMemoryEvidenceStore};
use forge_event::{EventBus, InMemoryEventBus, Topic};
use forge_exec::{EchoTool, PermissionPolicy, ToolRouter};
use forge_session::SessionStore;
use forge_sdk::{ForgeSdk, Orchestrator};
use forge_task::{AcceptanceCriterion, Task, TaskStore};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use forge_product_instance::{ProductInstanceStore as _, TemplateRegistry as _};
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
        }
    }
}

// ==================== 错误 ====================

pub struct ApiError(StatusCode, String);

impl From<ForgeError> for ApiError {
    fn from(e: ForgeError) -> Self {
        let status = match &e {
            ForgeError::NotFound(_) => StatusCode::NOT_FOUND,
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
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).map_err(ApiError::from)?;
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
        planner: None,
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

// ==================== CORS（API-004）====================

fn cors_layer() -> tower_http::cors::CorsLayer {
    let origins = std::env::var("FORGE_CORS_ORIGINS").unwrap_or_default();
    if origins.is_empty() {
        return tower_http::cors::CorsLayer::permissive();
    }
    let allow: Vec<_> = origins
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    tower_http::cors::CorsLayer::new().allow_origin(allow)
}

// ==================== 路由 ====================

pub fn app() -> Router { app_with_state(AppState::in_memory()) }

pub fn app_with_state(st: AppState) -> Router {
    Router::new()
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
        .route("/", get(ui_index))
        .route("/ui/sessions", get(ui_sessions))
        .route("/ui/evidence", get(ui_evidence))
        .layer(cors_layer())
        .with_state(st)
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
            }
        }
        Err(_) => { println!("storage: in-memory"); AppState::in_memory() }
    };
    let app = app_with_state(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    println!("forge-server listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
