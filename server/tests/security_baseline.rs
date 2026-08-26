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

// ===== GA-FIX-1：spawn 级验证（退出码 78 + 逃生门）=====

use std::process::{Command, Stdio};

/// 构造一个干净环境的服务器子进程命令（隔离父进程的 FORGE_* 变量）。
fn server_cmd(port: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_forge-server"));
    c.env("FORGE_HOST", "0.0.0.0")
        .env("FORGE_PORT", port)
        .env_remove("FORGE_API_KEY")
        .env_remove("FORGE_INSECURE_LOCAL")
        .env_remove("FORGE_PG_URL")
        .env_remove("FORGE_CORS_ORIGINS");
    c
}

#[test]
fn refusal_exit_code_is_78() {
    let out = server_cmd("18098")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .output()
        .expect("spawn forge-server");
    assert_eq!(out.status.code(), Some(78), "SEC-001 拒绝必须退出码 78");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("SEC-001"), "stderr 应含 SEC-001 前缀，实际: {err}");
}

#[test]
fn escape_hatch_allows_startup() {
    let mut child = server_cmd("18099")
        .env("FORGE_INSECURE_LOCAL", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn forge-server with escape hatch");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let still_running = match child.try_wait() {
        Ok(None) => true,
        Ok(Some(st)) => {
            eprintln!("early exit: {st}");
            false
        }
        Err(_) => false,
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(still_running, "逃生门应允许进程持续运行（未被 78 拒绝）");
}
