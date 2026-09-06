//! V6.0 BILL-002：费率与账单（build_v60b.md 冻结测试，PG 门控）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::billing::{generate_bill, record_usage, set_rate, BillDoc};
use forge_server::auth::TenantKeyStore as _;
use forge_server::{app_with_state, AppState};
use forge_storage::connect_and_migrate;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

/// 各测试只清自己的租户域——并行测试互不踩踏（全表清理曾导致
/// 重算竞态：A 的两次 generate 之间被 B 的清理删了流水）。
async fn pool(tenant: &str) -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    sqlx::query("DELETE FROM usage_events WHERE tenant_id = $1").bind(tenant).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM bills WHERE tenant_id = $1").bind(tenant).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rates WHERE tenant_id = $1").bind(tenant).execute(&pool).await.unwrap();
    Some(pool)
}

fn window() -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    let to = chrono::Utc::now();
    (to - chrono::Duration::hours(1), to + chrono::Duration::hours(1))
}

/// 冻结测试（BILL-002）：同一 usage 流水重算两次 → doc 字节一致 + doc_hash 一致。
#[tokio::test]
async fn bill_recompute_is_byte_identical() {
    let Some(pool) = pool("t-bill").await else { return };
    for (k, q) in [("task_count", 3), ("storage_bytes", 2048)] {
        record_usage(&pool, "t-bill", k, q, serde_json::json!({})).await;
    }
    set_rate(&pool, "t-bill", "task_count", 10_000, "CNY").await.unwrap();
    set_rate(&pool, "t-bill", "storage_bytes", 1, "CNY").await.unwrap();
    let (from, to) = window();

    let (id1, hash1, doc1) = generate_bill(&pool, "t-bill", from, to).await.unwrap();
    let (id2, hash2, doc2) = generate_bill(&pool, "t-bill", from, to).await.unwrap();
    assert_eq!(id1, id2, "幂等 upsert 必须同一账单行");
    assert_eq!(hash1, hash2, "doc_hash 必须一致");
    assert_eq!(
        serde_json::to_string(&doc1).unwrap(),
        serde_json::to_string(&doc2).unwrap(),
        "重算两次 doc 必须字节一致"
    );
    assert_eq!(doc1.total_micros, 3 * 10_000 + 2048);
    sqlx::query("DELETE FROM bills WHERE tenant_id='t-bill'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rates WHERE tenant_id='t-bill'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM usage_events WHERE tenant_id='t-bill'").execute(&pool).await.unwrap();
}

/// 冻结测试（BILL-002）：无费率 kind 金额 0 且进入 unrated 列。
#[tokio::test]
async fn unrated_kind_billed_zero() {
    let Some(pool) = pool("t-unr").await else { return };
    record_usage(&pool, "t-unr", "task_count", 2, serde_json::json!({})).await;
    // 故意不给 task_count 配费率
    let (from, to) = window();
    let (_, _, doc) = generate_bill(&pool, "t-unr", from, to).await.unwrap();
    let line = doc.lines.iter().find(|l| l.kind == "task_count").expect("task_count 行必须在");
    assert_eq!(line.amount_micros, 0);
    assert_eq!(line.unit_price_micros, 0);
    assert!(doc.unrated.contains(&"task_count".to_string()), "unrated 列: {:?}", doc.unrated);
    sqlx::query("DELETE FROM bills WHERE tenant_id='t-unr'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM usage_events WHERE tenant_id='t-unr'").execute(&pool).await.unwrap();
}

/// 冻结测试（BILL-002）：tenant-a 配费率不影响 tenant-b（b 为 unrated 0）。
#[tokio::test]
async fn rates_per_tenant_isolated() {
    let Some(pool) = pool("tenant-a").await else { return };
    sqlx::query("DELETE FROM usage_events WHERE tenant_id = 'tenant-b'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM bills WHERE tenant_id = 'tenant-b'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rates WHERE tenant_id = 'tenant-b'").execute(&pool).await.unwrap();
    set_rate(&pool, "tenant-a", "task_count", 500, "CNY").await.unwrap();
    for t in ["tenant-a", "tenant-b"] {
        record_usage(&pool, t, "task_count", 2, serde_json::json!({})).await;
    }
    let (from, to) = window();
    let (_, _, doc_a) = generate_bill(&pool, "tenant-a", from, to).await.unwrap();
    let (_, _, doc_b) = generate_bill(&pool, "tenant-b", from, to).await.unwrap();
    assert_eq!(doc_a.total_micros, 1000, "a 用自己的费率");
    assert_eq!(doc_b.total_micros, 0, "b 无费率必须 0");
    assert!(doc_b.unrated.contains(&"task_count".to_string()));
    sqlx::query("DELETE FROM bills WHERE tenant_id LIKE 'tenant-%'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rates WHERE tenant_id LIKE 'tenant-%'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM usage_events WHERE tenant_id LIKE 'tenant-%'").execute(&pool).await.unwrap();
}

/// 冻结测试（BILL-002）：生成 → CSV 导出 → 表头/行序/金额正确；JSON 导出与 doc 一致。
#[tokio::test]
async fn bill_export_csv_roundtrip() {
    let Some(pool) = pool("t-csv").await else { return };
    set_rate(&pool, "t-csv", "storage_bytes", 2, "CNY").await.unwrap();
    set_rate(&pool, "t-csv", "task_count", 100, "CNY").await.unwrap();
    record_usage(&pool, "t-csv", "task_count", 5, serde_json::json!({})).await;
    record_usage(&pool, "t-csv", "storage_bytes", 10, serde_json::json!({})).await;
    let (from, to) = window();
    let (id, _, _) = generate_bill(&pool, "t-csv", from, to).await.unwrap();

    let mut st = AppState::in_memory();
    st.pool = Some(pool.clone());
    let app = app_with_state(st);

    let get = |app: axum::Router, uri: String| async move {
        let res = app.oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, bytes)
    };

    let (status, csv_bytes) = get(app.clone(), format!("/admin/bills/{id}/export?format=csv")).await;
    assert_eq!(status, StatusCode::OK);
    let csv = String::from_utf8(csv_bytes.to_vec()).unwrap();
    assert!(
        csv.starts_with("kind,quantity,unit_price_micros,amount_micros,currency\n"),
        "表头: {csv}"
    );
    let lines: Vec<&str> = csv.trim().split('\n').skip(1).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "storage_bytes,10,2,20,CNY", "按 kind 字典序: {csv}");
    assert_eq!(lines[1], "task_count,5,100,500,CNY");

    let (status, json_bytes) = get(app, format!("/admin/bills/{id}/export?format=json")).await;
    assert_eq!(status, StatusCode::OK);
    let doc: BillDoc = serde_json::from_slice(&json_bytes).unwrap();
    assert_eq!(doc.total_micros, 520);
    sqlx::query("DELETE FROM bills WHERE tenant_id='t-csv'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM rates WHERE tenant_id='t-csv'").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM usage_events WHERE tenant_id='t-csv'").execute(&pool).await.unwrap();
}

/// 冻结测试（BILL-002）：非 default 租户调 /admin/* → 403。
#[tokio::test]
async fn billing_admin_requires_default_tenant() {
    let Some(pool) = pool("t-admin").await else { return };
    // 启用鉴权并发放 other-tenant 租户钥
    let store = forge_server::auth::InMemoryTenantKeyStore::default();
    let raw = store.issue("other-tenant").await.unwrap();
    let mut st = AppState::in_memory();
    st.pool = Some(pool.clone());
    st.tenant_keys = Arc::new(store);
    st.auth = forge_server::auth::AuthConfig { api_key: Some("legacy".into()) };
    let app = app_with_state(st);

    let send_post = |app: axum::Router, token: String, uri: &'static str, body: String| async move {
        let res = app
            .oneshot(
                Request::post(uri)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        res.status()
    };
    let rate_body = serde_json::json!({"tenant_id": "x", "kind": "k", "unit_price_micros": 1}).to_string();
    let bill_body = serde_json::json!({"tenant_id": "x", "period_from": "2026-01-01T00:00:00Z", "period_to": "2026-02-01T00:00:00Z"}).to_string();

    assert_eq!(
        send_post(app.clone(), raw.clone(), "/admin/rates", rate_body).await,
        StatusCode::FORBIDDEN,
        "非 default 租户设费率必须 403"
    );
    assert_eq!(
        send_post(app, raw, "/admin/bills/generate", bill_body).await,
        StatusCode::FORBIDDEN,
        "非 default 租户生成账单必须 403"
    );
}
