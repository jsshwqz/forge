//! SRV-FIX-001：server 端点测试覆盖整改（R7-005）。
//!
//! 测试矩阵（roadmap_b.md §3.1）：
//! | # | 场景 | 断言 |
//! |---|---|---|
//! | 1 | GET /health | 200 + "ok" |
//! | 2 | POST /tasks 合法 goal | 200/201 + task_id |
//! | 3 | GET /tasks/{已存在} | 200 + 字段一致 |
//! | 4 | GET /tasks/{不存在} | 404 |
//! | 5 | GET /sessions/{不存在} | 404 |
//! | 6 | GET /sessions/{已存在} | 200 + state |

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::{app_with_state, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> axum::Router {
    app_with_state(AppState::in_memory())
}

async fn send(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, serde_json::Value) {
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

// ---- 场景 1: GET /health ----

#[tokio::test]
async fn test_01_health_returns_ok() {
    let (status, body) = send(
        app(),
        Request::get("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "forge-server");
}

// ---- 场景 2: POST /tasks ----

#[tokio::test]
async fn test_02_create_task_returns_id() {
    let (status, body) = send(
        app(),
        Request::post("/tasks")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "goal": "SRV-FIX-001 test goal",
                    "acceptance": [{
                        "id": "AC-1",
                        "description": "test",
                        "check": {"FileExists": "out.txt"}
                    }]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 200 or 201, got {status}"
    );
    assert!(body["id"].as_str().is_some(), "response must contain task_id");
    assert_eq!(body["status"], "Pending");
}

// ---- 场景 3: GET /tasks/{已存在id} ----

#[tokio::test]
async fn test_03_get_existing_task_fields_match() {
    let app = app();

    // 先创建
    let (_, created) = send(
        app.clone(),
        Request::post("/tasks")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "goal": "field-match-test",
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    // 再查询
    let (status, got) = send(
        app,
        Request::get(format!("/tasks/{id}")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["goal"], "field-match-test");
    assert_eq!(got["status"], "Pending");
    assert_eq!(got["id"], id);
}

// ---- 场景 4: GET /tasks/{不存在id} ----

#[tokio::test]
async fn test_04_get_nonexistent_task_returns_404() {
    let (status, _) = send(
        app(),
        Request::get("/tasks/task_nonexistent_999")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---- 场景 5: GET /sessions/{不存在id} ----

#[tokio::test]
async fn test_05_get_nonexistent_session_returns_404() {
    let (status, _) = send(
        app(),
        Request::get("/sessions/session_nonexistent_999")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---- 场景 6: GET /sessions/{已存在id} ----

#[tokio::test]
async fn test_06_get_existing_session_has_state() {
    let st = AppState::in_memory();
    // 通过 SDK 创建会话
    let session = st.sdk.create_session(forge_core::TaskId::new_task_id()).await.unwrap();

    let (status, got) = send(
        app_with_state(st),
        Request::get(format!("/sessions/{}", session.id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(got["state"].as_str().is_some(), "state field must exist");
    assert_eq!(got["state"], "Active");
}
