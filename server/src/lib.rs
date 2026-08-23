//! forge-server：Aion Forge 2.0 HTTP API 层。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use forge_core::{ForgeError, SessionId, TaskId};
use forge_evidence::InMemoryEvidenceStore;
use forge_exec::{EchoTool, PermissionPolicy, ToolRouter};
use forge_sdk::ForgeSdk;
use forge_task::{AcceptanceCriterion, Task, TaskStore};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use serde::{Deserialize};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub sdk: ForgeSdk,
    pub workspaces: Arc<WorkspaceManager>,
}

impl AppState {
    /// 注入自定义存储。
    pub fn new(tasks: Arc<dyn TaskStore>, sessions: Arc<dyn forge_session::SessionStore>) -> Self {
        Self { sdk: ForgeSdk::from_stores(tasks, sessions), workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()) }
    }
    /// 注入自定义存储（测试/PG 切换用）。
    pub fn with_stores(tasks: Arc<dyn TaskStore>, sessions: Arc<dyn forge_session::SessionStore>) -> Self {
        Self { sdk: ForgeSdk::from_stores(tasks, sessions), workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()) }
    }
    pub fn in_memory() -> Self {
        Self {
            sdk: ForgeSdk::in_memory(),
            workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
        }
    }
}

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
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

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
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 { 30 }

struct DemoAllowAll;
impl PermissionPolicy for DemoAllowAll {
    fn check(&self, _level: forge_exec::PermissionLevel, _ctx: &forge_exec::PolicyContext) -> forge_core::ForgeResult<()> {
        Ok(())
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok","service":"forge-server","version":env!("CARGO_PKG_VERSION")}))
}

async fn create_task(State(st): State<AppState>, Json(req): Json<CreateTaskRequest>) -> Result<Json<Task>, ApiError> {
    let task = st.sdk.create_task(req.goal, req.constraints, req.acceptance).await?;
    Ok(Json(task))
}

async fn get_task(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<Task>, ApiError> {
    let task = st.sdk.get_task(&TaskId::from(id)).await?;
    Ok(Json(task))
}

async fn list_tasks(State(st): State<AppState>) -> Json<serde_json::Value> {    let ids = st.sdk.list_tasks().await.unwrap_or_default();    Json(serde_json::json!({ "count": ids.len(), "ids": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>() }))}
async fn get_session(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<forge_session::Session>, ApiError> {
    let s = st.sdk.sessions().get(&SessionId::from(id)).await?;
    Ok(Json(s))
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
        evidence: Arc::new(InMemoryEvidenceStore::default()),
        workspace: st.workspaces.clone(),
        timeout: Duration::from_secs(req.timeout_secs),
    };

    let orch = forge_sdk::Orchestrator {
        capability: "echo".into(),
        timeout: Duration::from_secs(req.timeout_secs),
    };

    let report = st.sdk.run_end_to_end(&task.id, &deps, &orch).await.map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "task_id": report.task_id.to_string(),
        "final_status": format!("{:?}", report.final_status),
        "gate_passed": report.gate.passed,
        "execution_waves": report.execution.waves,
        "steps_completed": report.execution.completed.len(),
        "verifications": report.verifications.iter().map(|v| {
            serde_json::json!({"criterion_id": v.criterion_id,"verdict": format!("{:?}", v.verdict),"reason": v.reason})
        }).collect::<Vec<_>>(),
        "evidence_count": report.evidence_ids.len(),
    })))
}

pub fn app() -> Router { app_with_state(AppState::in_memory()) }

pub fn app_with_state(st: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/tasks", post(create_task))
        .route("/tasks/:id", get(get_task))
        .route("/tasks", get(list_tasks)).route("/sessions/:id", get(get_session))
        .route("/orchestrate", post(orchestrate))
        .with_state(st)
}

pub async fn run_from_env() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .try_init().ok();

    let port: u16 = std::env::var("FORGE_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8080);
    let state = match std::env::var("FORGE_PG_URL") {
        Ok(url) => {
            println!("storage: PostgreSQL");
            AppState {
                sdk: ForgeSdk::postgres(&url).await?,
                workspaces: Arc::new(WorkspaceManager::new(std::env::temp_dir().join("forge-ws")).unwrap()),
            }
        }
        Err(_) => { println!("storage: in-memory"); AppState::in_memory() }
    };
    let app = app_with_state(state);
    let addr = std::net::SocketAddr::from(([127,0,0,1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| panic!("bind: {e}"));
    println!("forge-server listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
