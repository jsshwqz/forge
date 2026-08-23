//! forge-server：Aion Forge 2.0 HTTP API 层（第二阶段，施工包 B-02 / 技术栈冻结 axum）。
//!
//! 第一批端点（最小可用面）：
//! - `GET  /health`                 健康检查
//! - `POST /tasks`                  创建任务（InMemoryTaskStore）
//! - `GET  /tasks/{id}`             查询任务
//! - `GET /sessions/{id}`           查询会话
//!
//! 持久化当前复用第一阶段内存实现；PH2-001 接入 PostgreSQL 后仅替换 State 组装，
//! 路由层不变（AP-015：Product/Server 不修改 Core 内部实现）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use forge_core::{ForgeError, SessionId, TaskId};
use forge_session::{InMemorySessionStore, SessionStore};
use forge_task::{AcceptanceCriterion, InMemoryTaskStore, Task, TaskStore};
use std::sync::Arc;

/// 应用共享状态。
#[derive(Clone)]
pub struct AppState {
    pub tasks: Arc<InMemoryTaskStore>,
    pub sessions: Arc<InMemorySessionStore>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tasks: Arc::new(InMemoryTaskStore::default()),
            sessions: Arc::new(InMemorySessionStore::default()),
        }
    }
}

/// API 错误：把 ForgeError 映射为 HTTP 状态码。
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

/// 创建任务请求体。
#[derive(serde::Deserialize)]
pub struct CreateTaskRequest {
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "forge-server",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn create_task(
    State(st): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<Task>, ApiError> {
    let task = st
        .tasks
        .create(req.goal, req.constraints, req.acceptance)
        .await?;
    Ok(Json(task))
}

async fn get_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, ApiError> {
    let task = st.tasks.get(&TaskId::from(id)).await?;
    Ok(Json(task))
}

async fn get_session(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<forge_session::Session>, ApiError> {
    let s = st.sessions.get(&SessionId::from(id)).await?;
    Ok(Json(s))
}

/// 组装路由。main 与测试共用。
pub fn app() -> Router {
    app_with_state(AppState::default())
}

/// 用给定状态组装路由（测试可注入预置数据）。
pub fn app_with_state(st: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/tasks", post(create_task))
        .route("/tasks/:id", get(get_task))
        .route("/sessions/:id", get(get_session))
        .with_state(st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn send(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, json)
    }

    #[tokio::test]
    async fn test_health() {
        let (status, body) = send(app(), Request::get("/health").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "forge-server");
    }

    #[tokio::test]
    async fn test_create_and_get_task() {
        let app = app();
        // 创建
        let (status, created) = send(
            app.clone(),
            Request::post("/tasks")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "goal": "生成 output.txt",
                        "acceptance": [{
                            "id": "AC-1",
                            "description": "文件存在",
                            "check": {"FileExists": "output.txt"}
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["status"], "Pending");

        // 查询
        let id = created["id"].as_str().unwrap().to_string();
        let (status, got) = send(
            app,
            Request::get(format!("/tasks/{}", id)).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(got["goal"], "生成 output.txt");
    }

    #[tokio::test]
    async fn test_get_task_not_found() {
        let (status, _) = send(
            app(),
            Request::get("/tasks/task_nonexistent").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let (status, _) = send(
            app(),
            Request::get("/sessions/session_x").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_session_found() {
        let st = AppState::default();
        let session = st.sessions.create(forge_core::TaskId::new_task_id()).await.unwrap();

        let (status, got) = send(
            app_with_state(st),
            Request::get(format!("/sessions/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(got["state"], "Active");
    }
}
