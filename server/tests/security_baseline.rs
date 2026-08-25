//! SEC-001 生产安全基线测试。
//!
//! 覆盖：
//! - CORS 默认关闭（无 FORGE_CORS_ORIGINS 时响应不带 CORS 头）
//! - 启用 FORGE_API_KEY 后：无/错 Bearer → 401，正确 Bearer → 200，/health 恒放行
//! - 本地模式（未设 key）全放行
//! - security_gate：非 loopback 监听未配 key 必须拒绝启动
//!
//! 注意：env 为进程级全局，本文件所有场景合并进单个测试函数串行执行。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use forge_server::{app_with_state, security_gate, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn status_of(app: axum::Router, req: Request<Body>) -> (StatusCode, String, String) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let acao = res
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, acao, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn sec001_baseline_matrix() {
    // ===== 场景 A：默认（无 key、无 CORS 白名单）——本地模式全放行、CORS 关闭 =====
    std::env::remove_var("FORGE_API_KEY");
    std::env::remove_var("FORGE_CORS_ORIGINS");
    let app = app_with_state(AppState::in_memory());

    let (st, acao, _) = status_of(
        app.clone(),
        Request::get("/tasks").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "本地模式应放行");
    assert!(acao.is_empty(), "CORS 默认必须关闭，不应出现 allow-origin 头");

    // /health 同样开放
    let (st, _, _) = status_of(
        app.clone(),
        Request::get("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    drop(app);

    // ===== 场景 B：启用 FORGE_API_KEY =====
    std::env::set_var("FORGE_API_KEY", "sec-baseline-key");
    let app = app_with_state(AppState::in_memory());

    // 无 Bearer → 401
    let (st, _, body) = status_of(
        app.clone(),
        Request::get("/tasks").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert!(!body.contains("sec-baseline-key"), "401 响应不得回显密钥");

    // 错误 Bearer → 401
    let (st, _, _) = status_of(
        app.clone(),
        Request::get("/tasks")
            .header(header::AUTHORIZATION, "Bearer wrong-key")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // 正确 Bearer → 200
    let (st, _, _) = status_of(
        app.clone(),
        Request::get("/tasks")
            .header(header::AUTHORIZATION, "Bearer sec-baseline-key")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // /health 永远放行（无需 Bearer）
    let (st, _, _) = status_of(
        app.clone(),
        Request::get("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 401 统一文案不泄露原因（含错误 Bearer 时同样不回显 key）
    drop(app);
    std::env::remove_var("FORGE_API_KEY");

    // ===== 场景 C：security_gate 矩阵 =====
    assert!(security_gate("127.0.0.1", None).is_ok(), "loopback 免 key");
    assert!(security_gate("localhost", None).is_ok());
    assert!(security_gate("::1", None).is_ok());
    assert!(security_gate("0.0.0.0", None).is_err(), "非 loopback 无 key 必须拒绝");
    assert!(security_gate("192.168.1.10", Some("  ")).is_err(), "空白 key 视同未配置");
    assert!(security_gate("0.0.0.0", Some("real-key")).is_ok(), "有 key 放行对外监听");
}
