//! V5-FIX-2a TEN-002：多租户鉴权接线（HTTP 面）。
//!
//! 冻结测试名：unknown_key_401（auth.rs 单元侧另含
//! legacy_key_maps_default / tenant_key_resolves）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::auth::{AuthConfig, InMemoryTenantKeyStore, TenantKeyStore as _};
use forge_server::{app_with_state, AppState};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

async fn status_of(app: axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn get_tasks(bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::get("/tasks");
    if let Some(token) = bearer {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    b.body(Body::empty()).unwrap()
}

/// 冻结测试：已配钥时未知 Bearer → 401，且响应不回显密钥。
#[tokio::test]
async fn unknown_key_401() {
    let mut st = AppState::in_memory();
    st.auth = AuthConfig { api_key: Some("legacy-key".into()) };
    let app = app_with_state(st);

    // 未知 Bearer → 401
    let (status, body) = status_of(app.clone(), get_tasks(Some("wrong-key"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(!body.to_string().contains("legacy-key"), "401 不得回显密钥");

    // 无 Bearer → 401（已配钥时缺失凭据不放行）
    let (status, _) = status_of(app.clone(), get_tasks(None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// 租户钥与 legacy 单钥在 HTTP 面均解析成功（200），且下游拿到租户身份。
#[tokio::test]
async fn tenant_key_http_resolves_and_legacy_falls_back() {
    let mut st = AppState::in_memory();
    let store = InMemoryTenantKeyStore::default();
    let raw = store.issue("acme").await.unwrap();
    st.tenant_keys = Arc::new(store);
    st.auth = AuthConfig { api_key: Some("legacy-key".into()) };
    let app = app_with_state(st);

    // 租户钥命中 → 200（走 tenant_keys 哈希解析）
    let (status, _) = status_of(app.clone(), get_tasks(Some(&raw))).await;
    assert_eq!(status, StatusCode::OK, "租户钥必须解析通过");

    // legacy 单钥 → 200（映射 DEFAULT_TENANT）
    let (status, _) = status_of(app.clone(), get_tasks(Some("legacy-key"))).await;
    assert_eq!(status, StatusCode::OK, "legacy 单钥必须回退放行");

    // /health 永远放行（无需 Bearer）
    let (status, _) = status_of(
        app,
        Request::get("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
