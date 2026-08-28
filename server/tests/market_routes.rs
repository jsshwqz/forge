//! V5.0 MKT-001/002: 市场目录 API 集成测试
//!
//! 覆盖：
//! - GET /market/capabilities 公开访问（免鉴权）
//! - 分页 clamp 到 [1, 100]
//! - 响应不含 entry 细节
//! - POST /market/install 注册能力
//! - 幂等安装

use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use forge_server::{app_with_state, AppState};
use forge_cap::{Capability, CapabilityKind, CapabilityStatus};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> axum::Router {
    app_with_state(AppState::in_memory())
}

async fn send_json(
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

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn catalog_public_no_auth() {
    let app = app();
    
    // 市场目录应返回 200（免鉴权）
    let (status, _body) = send_json(app, get("/market/capabilities")).await;
    assert_eq!(status, StatusCode::OK, "market catalog should be publicly accessible");
}

#[tokio::test]
async fn pagination_clamped() {
    let app = app();
    
    // per_page=9999 应该被 clamp 到 100
    let (status, body) = send_json(
        app,
        Request::get("/market/capabilities?per_page=9999").body(Body::empty()).unwrap(),
    ).await;
    
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array());
    let items = body["items"].as_array().unwrap();
    assert!(items.len() <= 100, "per_page should be clamped to 100");
}

#[tokio::test]
async fn no_entry_leak() {
    let mut state = AppState::in_memory();
    
    // 插入一个测试能力
    state.capabilities.register(Capability {
        id: "cap-test-001".into(),
        name: "echo".into(),
        kind: CapabilityKind::Tool,
        version: "0.1.0".into(),
        entry: "echo_tool_v0.1.0".into(),
        status: CapabilityStatus::Active,
        permission: forge_exec::PermissionLevel::ReadOnly,
    }).await.unwrap();
    
    let app = app_with_state(state);
    
    let (_status, body) = send_json(app, get("/market/capabilities")).await;
    let items = body["items"].as_array().unwrap();
    
    assert!(!items.is_empty(), "should have at least one item");
    
    // 验证响应不含 entry 字段
    let item = &items[0];
    assert!(item["name"].is_string());
    assert!(item["version"].is_string());
    // entry 字段不应出现在市场响应中
    assert!(item.as_object().unwrap().get("entry").is_none(), "entry should not leak in market response");
}

#[tokio::test]
async fn install_registers_active() {
    let mut state = AppState::in_memory();
    
    // 插入源能力
    state.capabilities.register(Capability {
        id: "cap-source-001".into(),
        name: "echo".into(),
        kind: CapabilityKind::Tool,
        version: "0.1.0".into(),
        entry: "echo_tool".into(),
        status: CapabilityStatus::Registered,
        permission: forge_exec::PermissionLevel::ReadOnly,
    }).await.unwrap();
    
    let app = app_with_state(state);
    
    // 安装能力
    let (status, body) = send_json(
        app,
        post("/market/install", serde_json::json!({
            "name": "echo",
            "version": "0.1.0"
        })),
    ).await;
    
    assert_eq!(status, StatusCode::OK, "install should return 200");
    assert!(body["installed"].as_bool().unwrap_or(false), "should confirm installation");
    assert!(body["id"].is_string(), "should return capability id");
}

#[tokio::test]
async fn idempotent_reinstall() {
    let mut state = AppState::in_memory();
    
    // 插入源能力
    state.capabilities.register(Capability {
        id: "cap-idem-001".into(),
        name: "echo".into(),
        kind: CapabilityKind::Tool,
        version: "0.1.0".into(),
        entry: "echo_tool".into(),
        status: CapabilityStatus::Registered,
        permission: forge_exec::PermissionLevel::ReadOnly,
    }).await.unwrap();
    
    let app = app_with_state(state);
    
    // 第一次安装
    let (_s1, b1) = send_json(
        app.clone(),
        post("/market/install", serde_json::json!({"name": "echo", "version": "0.1.0"})),
    ).await;
    
    // 第二次安装（幂等）
    let (_s2, b2) = send_json(
        app,
        post("/market/install", serde_json::json!({"name": "echo", "version": "0.1.0"})),
    ).await;
    
    assert_eq!(b1["id"], b2["id"], "idempotent install should return same id");
    assert!(b2["installed"].as_bool().unwrap_or(false), "should confirm idempotent installation");
}

#[tokio::test]
async fn missing_capability_returns_404() {
    let app = app();
    
    let (status, _body) = send_json(
        app,
        post("/market/install", serde_json::json!({"name": "nonexistent", "version": "1.0.0"})),
    ).await;
    
    assert_eq!(status, StatusCode::NOT_FOUND, "missing capability should return 404");
}
