//! V6.0 BILL-001：计量流水（build_v60b.md 冻结测试，PG 门控）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::{app_with_state, AppState};
use forge_storage::connect_and_migrate;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn pg_app() -> Option<axum::Router> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    sqlx::query("DELETE FROM usage_events").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM task_queue").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM session_events").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM sessions").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM tasks").execute(&pool).await.unwrap();
    let mut st = AppState::in_memory();
    st.pool = Some(pool);
    Some(app_with_state(st))
}

/// 冻结测试（BILL-001）：一次 orchestrate 后 task_count=1、storage_bytes≥0
/// （无 LLM 配置时不产生 token 事件）。
#[tokio::test]
async fn usage_three_dimensions_recorded() {
    let Some(app) = pg_app().await else { return };
    let cmd =
        if cfg!(target_os = "windows") { "echo hi> out.txt" } else { "echo hi > out.txt" };
    let (status, _body) = {
        let res = app.clone()
            .oneshot(
                Request::post("/orchestrate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "goal": "usage probe",
                            "acceptance": [
                                {"id":"AC-1","description":"write file","check":{"Command": cmd}}
                            ],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
    };
    assert_eq!(status, StatusCode::OK);

    // 查询计量面
    let (status, body) = {
        let res = app
            .oneshot(Request::get("/admin/usage?tenant=default").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
    };
    assert_eq!(status, StatusCode::OK, "计量查询必须 200: {body}");
    let items = body["items"].as_array().expect("items 数组");
    let find = |k: &str| items.iter().find(|i| i["kind"] == k).cloned();
    let tc = find("task_count").expect("必须有 task_count");
    assert_eq!(tc["sum"], 1, "task_count 必须恰 1: {body}");
    let sb = find("storage_bytes").expect("必须有 storage_bytes");
    assert!(sb["sum"].as_i64().unwrap() >= 0, "storage_bytes ≥ 0");
    assert!(find("llm_token").is_none(), "无 LLM 配置不应有 token 事件");
}

/// 冻结测试（BILL-001）：按 kind 聚合正确。
#[tokio::test]
async fn summarize_aggregates_by_kind() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    sqlx::query("DELETE FROM usage_events").execute(&pool).await.unwrap();
    let now = chrono::Utc::now();
    let from = now - chrono::Duration::hours(1);
    let to = now + chrono::Duration::hours(1);
    for (k, q) in [("task_count", 1), ("task_count", 1), ("storage_bytes", 1024), ("llm_token", 55)] {
        forge_server::billing::record_usage(&pool, "t-agg", k, q, serde_json::json!({})).await;
    }
    let rows = forge_server::billing::summarize_usage(&pool, "t-agg", from, to).await.unwrap();
    assert_eq!(rows.len(), 3, "三 kind 各一行: {rows:?}");
    assert_eq!(rows[0], ("llm_token".into(), 55));
    assert_eq!(rows[1], ("storage_bytes".into(), 1024));
    assert_eq!(rows[2], ("task_count".into(), 2));
    sqlx::query("DELETE FROM usage_events WHERE tenant_id = 't-agg'")
        .execute(&pool)
        .await
        .unwrap();
}
