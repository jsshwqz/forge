//! G-V4.0 门禁集成测试（PROD-001/002 + OBS-002 + UI-001，全离线）。
//!
//! 覆盖契约 6.3：
//! - 实例化产品 → start → orchestrate 一次 → stop → deprecate 全程状态机合法；
//! - /metrics 四组计数器与实际执行一致；
//! - 控制台三页面可访问（数据源为 V3.0 只读 API）。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use forge_server::{app_with_state, AppState};
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

async fn send_raw(app: axum::Router, req: Request<Body>) -> (StatusCode, String, String) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, ct, String::from_utf8_lossy(&bytes).into_owned())
}

fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

/// 构造一个最小合法模板的 JSON 值。
fn template_json(id: &str) -> serde_json::Value {
    use forge_agent::AgentRole;
    use forge_core::ProductId;
    use forge_product::{CapabilityRef, ProductManifest, ProductTemplate};
    let tpl = ProductTemplate {
        id: id.into(),
        name: "demo product".into(),
        parameters: vec![],
        manifest_skeleton: ProductManifest {
            id: ProductId::new_product_id(),
            name: "demo".into(),
            version: "0.1.0".into(),
            description: "demo {{product_name}}".into(),
            capabilities: vec![CapabilityRef {
                capability_name: "echo".into(),
                version: "0.1.0".into(),
                required: true,
            }],
            entry_agent_role: AgentRole::Orchestrator,
        },
    };
    serde_json::to_value(tpl).unwrap()
}

#[tokio::test]
async fn g_v40_full_lifecycle_e2e() {
    let app = app();

    // ---- 模板发布：Reject 拒绝入库；Pass 通过 ----
    let bad_body = serde_json::json!({
        "template": template_json("tpl.gate"),
        "version": "1.0.0",
        "review_verdict": "Reject",
    });
    let (status, _) = send_json(app.clone(), post("/templates", bad_body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "Reject 不得入库");

    let (status, resp) = send_json(
        app.clone(),
        post(
            "/templates",
            serde_json::json!({
                "template": template_json("tpl.gate"),
                "version": "1.0.0",
                "review_verdict": "Pass",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["published"], "tpl.gate@1.0.0");

    // 列举可见
    let (status, list) = send_json(app.clone(), Request::get("/templates").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["count"], 1);

    // ---- 实例化 → Draft ----
    let (status, inst) = send_json(
        app.clone(),
        post(
            "/products/instantiate",
            serde_json::json!({
                "template_id": "tpl.gate",
                "version": "1.0.0",
                "name": "gate-e2e",
                "params": {},
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let iid = inst["instance_id"].as_str().unwrap().to_string();
    assert!(inst["state"] == "Draft");

    let (status, p) = send_json(
        app.clone(),
        Request::get(format!("/products/{iid}")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["state"], "Draft");

    // ---- start → Active ----
    let (status, p) = send_json(
        app.clone(),
        Request::post(format!("/products/{iid}/start")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["state"], "Active");

    // ---- orchestrate 一次（真实执行 echo 链路 + 命令验收）----
    let cmd =
        if cfg!(target_os = "windows") { "echo hi> out.txt" } else { "echo hi > out.txt" };
    let orch_body = serde_json::json!({
        "goal": "g-v40 e2e run",
        "acceptance": [{
            "id": "AC-1",
            "description": "write file",
            "check": {"Command": cmd},
        }],
    });
    let (status, rep) = send_json(app.clone(), post("/orchestrate", orch_body)).await;
    assert_eq!(status, StatusCode::OK, "orchestrate 应成功: {rep}");
    assert_eq!(rep["gate_passed"], true);

    // ---- stop → Stopped → deprecate → Deprecated ----
    let (status, p) = send_json(
        app.clone(),
        Request::post(format!("/products/{iid}/stop")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["state"], "Stopped");

    let (status, p) = send_json(
        app.clone(),
        Request::post(format!("/products/{iid}/deprecate")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["state"], "Deprecated");

    // ---- 终态不可复活 ----
    let (status, _) = send_json(
        app.clone(),
        Request::post(format!("/products/{iid}/start")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "Deprecated → Active 必须拒绝");
}

#[tokio::test]
async fn metrics_counters_match_actual_execution() {
    let app = app();

    // 基线：空指标
    let (status, _, body) = send_raw(
        app.clone(),
        Request::get("/metrics").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("tasks_total 0"));
    assert!(body.contains("executions_total 0"));

    // 一次建任务 + 一次编排（1 条验收通过、0 replan）
    let _ = send_json(
        app.clone(),
        post(
            "/tasks",
            serde_json::json!({"goal": "m", "constraints": [], "acceptance": []}),
        ),
    )
    .await;
    let cmd =
        if cfg!(target_os = "windows") { "echo ok> m.txt" } else { "echo ok > m.txt" };
    let orch_body = serde_json::json!({
        "goal": "metrics run",
        "acceptance": [{"id":"AC-1","description":"d","check":{"Command": cmd}}],
    });
    let (status, rep) = send_json(app.clone(), post("/orchestrate", orch_body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rep["replans_used"], 0);

    let (status, _, body) = send_raw(
        app.clone(),
        Request::get("/metrics").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("tasks_total 1"), "{body}");
    assert!(body.contains("executions_total 1"), "{body}");
    assert!(body.contains("verifications_pass 1"), "{body}");
    assert!(body.contains("verifications_fail 0"), "{body}");
    assert!(body.contains("replans_total 0"), "{body}");
}

#[tokio::test]
async fn console_pages_served_from_readonly_apis() {
    let app = app();
    for uri in ["/", "/ui/sessions", "/ui/evidence"] {
        let (status, ct, body) = send_raw(
            app.clone(),
            Request::get(uri).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ct.starts_with("text/html"), "{uri} content-type={ct}");
        assert!(body.contains("Aion Forge"), "{uri}");
    }
}
