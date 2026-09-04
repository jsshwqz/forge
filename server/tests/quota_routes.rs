//! V5-FIX-2b TEN-003：编排入口配额限流（HTTP 面，429 + 冻结错误体）。
//!
//! 冻结测试名：default_quota_applies / concurrency_exceeded_429 / daily_exceeded_429

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::quota::QuotaView;
use forge_server::{app_with_state, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn orch_body() -> serde_json::Value {
    let cmd =
        if cfg!(target_os = "windows") { "echo hi> out.txt" } else { "echo hi > out.txt" };
    serde_json::json!({
        "goal": "quota gate check",
        "acceptance": [{
            "id": "AC-1",
            "description": "write file",
            "check": {"Command": cmd},
        }],
    })
}

async fn send_json(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn orchestrate(app: axum::Router) -> (StatusCode, serde_json::Value) {
    send_json(
        app,
        Request::post("/orchestrate")
            .header("content-type", "application/json")
            .body(Body::from(orch_body().to_string()))
            .unwrap(),
    )
    .await
}

/// 默认配额 (4, 100)：首个编排请求必须放行。
#[tokio::test]
async fn default_quota_applies() {
    let (status, body) = orchestrate(app_with_state(AppState::in_memory())).await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "默认配额不应触发 429: {body}");
    assert_eq!(status, StatusCode::OK, "默认配额下编排应成功: {body}");
}

/// 超并发 → 429 {"error":{"code":"quota_concurrency"}}（真实触发，非纯函数测试）。
#[tokio::test]
async fn concurrency_exceeded_429() {
    let st = AppState::in_memory();
    st.quotas
        .set("default", QuotaView { max_concurrent: 0, daily_tasks: 100 })
        .await
        .unwrap();
    let (status, body) = orchestrate(app_with_state(st)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "超并发必须 429: {body}");
    assert_eq!(body["error"]["code"], "quota_concurrency", "冻结错误体: {body}");
}

/// 超日量 → 429 {"error":{"code":"quota_daily"}}（真实触发，非纯函数测试）。
#[tokio::test]
async fn daily_exceeded_429() {
    let st = AppState::in_memory();
    st.quotas
        .set("default", QuotaView { max_concurrent: 4, daily_tasks: 0 })
        .await
        .unwrap();
    let (status, body) = orchestrate(app_with_state(st)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "超日量必须 429: {body}");
    assert_eq!(body["error"]["code"], "quota_daily", "冻结错误体: {body}");
}
