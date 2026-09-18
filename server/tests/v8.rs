//! V8.0 CTX-001：工作区感知冻结测试（build_v80a.md）。
//!
//! 冻结测试名：context_injection_truncates / resume_workspace_reuses_dir。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::{app_with_state, build_workspace_context, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

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

fn write_cmd() -> &'static str {
    if cfg!(target_os = "windows") {
        "echo shared-content> marker.txt"
    } else {
        "echo shared-content > marker.txt"
    }
}

/// 冻结测试：工作区上下文注入块超 32KB 总量时截断并标 (truncated)。
#[test]
fn context_injection_truncates() {
    let tmp = tempfile::tempdir().unwrap();
    // 8 个 6KB 文件 = 48KB > 32KB 总量上限 → 截断。
    for i in 0..8 {
        std::fs::write(tmp.path().join(format!("f{i}.txt")), "x".repeat(6 * 1024)).unwrap();
    }
    let ctx = build_workspace_context(tmp.path());
    assert!(ctx.starts_with("=== Workspace context ===\n"), "注入块首行冻结");
    assert!(ctx.contains("(truncated)"), "超限必须截断标记: {}", ctx.len());
    assert!(ctx.len() <= forge_server::CONTEXT_MAX_BYTES + 32, "总量应贴近上限");
}

/// 冻结测试：workspace_task_id 续作既有工作区——任务 A 产出文件，任务 B 续作
/// 并由验收读回 A 的文件（验证工具 root 与验收 workdir 一致指向续作工作区）。
#[tokio::test]
async fn resume_workspace_reuses_dir() {
    let app = app_with_state(AppState::in_memory());

    // 任务 A：产生 marker.txt
    let (status_a, body_a) = send_json(
        app.clone(),
        Request::post("/orchestrate")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "goal": "produce marker file",
                    "acceptance": [{
                        "id": "AC-1",
                        "description": "write marker",
                        "check": {"Command": write_cmd()},
                    }],
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status_a, StatusCode::OK, "任务 A 应成功: {body_a}");
    let task_a = body_a["task_id"].as_str().unwrap().to_string();
    assert_eq!(body_a["gate_passed"], true);

    // 任务 B：续作 A，验收读取 A 的 marker.txt
    let (status_b, body_b) = send_json(
        app,
        Request::post("/orchestrate")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "goal": "resume previous workspace",
                    "workspace_task_id": task_a,
                    "acceptance": [{
                        "id": "AC-1",
                        "description": "read marker from previous workspace",
                        "check": {"FileContains": {"path": "marker.txt", "needle": "shared-content"}},
                    }],
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status_b, StatusCode::OK, "任务 B 续作应成功: {body_b}");
    assert_eq!(body_b["gate_passed"], true, "续作工作区应能读到既有产物: {body_b}");
}
