//! V6.0 MKT-101：发布者签名生态（build_v60.md 冻结测试，PG 门控）。
//!
//! 冻结测试名：
//! - unknown_publisher_rejected（未登记 publisher 发布 → 403）
//! - review_state_machine_transitions（合法迁移全通 + 非法迁移全拒）
//! - install_requires_valid_signature（错签名 release 走 install → 403）
//! - pinned_to_yanked_conflict_409（install 钉 yanked 版本 → 409；"*" 解析跳过 yanked → 404）
//!
//! IMPROVE-2R 补修（R7-023）：与 market_artifact.rs 同款并行隔离——
//! PID 前缀 test_id + 每测试唯一 publisher_id/name 后缀 + 过滤删而非全表 DELETE。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_cap::signing;
use forge_cap::{Capability, CapabilityKind, CapabilityRegistry as _, CapabilityStatus};
use forge_server::{app_with_state, AppState};
use forge_storage::connect_and_migrate;
use http_body_util::BodyExt;
use std::sync::atomic::{AtomicU64, Ordering};
use tower::ServiceExt;

/// 并行测试中每个调用拿到不同的数字，用于 publisher_id 和 name 后缀。
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成唯一测试 ID（如 "p12345-t0", "p12345-t1"...）。
/// 进程 ID 前缀确保跨次运行不撞主键（同一次进程内靠原子计数器天然不撞）。
fn test_id() -> String {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("p{}-t{n}", std::process::id())
}

/// 为单个测试创建独立的 AppState（带 PG pool）+ 唯一 test_id。
/// 返回 (router, pool, test_id)。
/// 不再用全表 DELETE——每测试用唯一前缀的 publisher_id/name，互不干扰。
async fn setup_app() -> Option<(axum::Router, sqlx::PgPool, String)> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let tid = test_id();
    let mut st = AppState::in_memory();
    st.pool = Some(pool.clone());
    Some((app_with_state(st), pool, tid))
}

async fn send(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn post_json(uri: &str, body: serde_json::Value, publisher: Option<&str>) -> Request<Body> {
    let mut b = Request::post(uri).header("content-type", "application/json");
    if let Some(pk) = publisher {
        b = b.header("Publisher-Key", pk);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

/// 冻结测试：未登记 publisher 发布 → 403。
#[tokio::test]
async fn unknown_publisher_rejected() {
    let Some((app, _pool, tid)) = setup_app().await else { return; };
    let ghost = format!("ghost-{tid}");
    let (status, body) = send(
        app,
        post_json(
            "/market/publish",
            serde_json::json!({
                "name": format!("cap-x-{tid}"), "version": "1.0.0",
                "package_hash": "deadbeef", "signature": "00".repeat(64),
            }),
            Some(&ghost),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "未登记 publisher 必须 403: {body}");
}

/// 冻结测试：合法迁移全通（pending→approved→published 自动、pending→rejected 终态）
/// + 非法迁移全拒（409）。
#[tokio::test]
async fn review_state_machine_transitions() {
    let Some((app, pool, tid)) = setup_app().await else { return; };

    // 登记发布者并发布两条 release（合法签名）
    let pub_a = format!("pub-a-{tid}");
    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&pub_a)
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();
    let name1 = format!("cap-mkt-{tid}");
    let name2 = format!("cap-mkt2-{tid}");
    let pkg = |name: &str, ver: &str, hash: &str| format!("{name}\n{ver}\n{hash}").into_bytes();

    let sig1 = signing::sign_package(&sk, &pkg(&name1, "1.0.0", "hash-a")).unwrap();
    let (status, b1) = send(
        app.clone(),
        post_json(
            "/market/publish",
            serde_json::json!({"name":&name1,"version":"1.0.0","package_hash":"hash-a","signature":sig1}),
            Some(&pub_a),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "合法签名发布必须 202: {b1}");
    assert_eq!(b1["review_status"], "pending");
    let rid1 = b1["release_id"].as_i64().unwrap();

    // 第二条（不同 hash 才能过 UNIQUE）走 rejected 分支
    let sig2 = signing::sign_package(&sk, &pkg(&name2, "1.0.0", "hash-b")).unwrap();
    let (_, b2) = send(
        app.clone(),
        post_json(
            "/market/publish",
            serde_json::json!({"name":&name2,"version":"1.0.0","package_hash":"hash-b","signature":sig2}),
            Some(&pub_a),
        ),
    )
    .await;
    let rid2 = b2["release_id"].as_i64().unwrap();

    // pending→approved（自动转 published）
    let (status, body) = send(
        app.clone(),
        post_json("/market/review", serde_json::json!({"release_id": rid1, "verdict": "approved"}), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pending→approved 必须通过: {body}");
    assert_eq!(body["review_status"], "published", "approved 必须自动转 published");

    // pending→rejected（终态）
    let (status, body) = send(
        app.clone(),
        post_json("/market/review", serde_json::json!({"release_id": rid2, "verdict": "rejected"}), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["review_status"], "rejected");

    // 非法迁移：published/rejected 再审 → 409；未知 verdict → 400
    for (rid, verdict) in [(rid1, "rejected"), (rid2, "approved")] {
        let (status, _) = send(
            app.clone(),
            post_json("/market/review", serde_json::json!({"release_id": rid, "verdict": verdict}), None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "终态再审必须 409");
    }
    let (status, _) = send(
        app,
        post_json("/market/review", serde_json::json!({"release_id": rid1, "verdict": "maybe"}), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 冻结测试：无签名/错签名 release 走 install → 403。
#[tokio::test]
async fn install_requires_valid_signature() {
    let Some((app, pool, tid)) = setup_app().await else { return; };

    // 注册者 + 一条签名与内容不符的 release（绕过入站验签，直插 DB 模拟历史脏数据）
    let pub_bad = format!("pub-bad-{tid}");
    let name = format!("cap-signed-{tid}");
    let (_, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&pub_bad)
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO releases (name, version, publisher_id, package_hash, signature, review_status) \
         VALUES ($1, '1.0.0', $2, 'hash-x', $3, 'published')",
    )
    .bind(&name)
    .bind(&pub_bad)
    .bind("ab".repeat(64)) // 格式合法但内容不符的签名
    .execute(&pool)
    .await
    .unwrap();

    // 注册同 name+version 的能力，走 install
    let mut st_state = AppState::in_memory();
    st_state.pool = Some(pool);
    st_state.capabilities.register(Capability {
        id: forge_core::new_capability_id(),
        name: name.clone(),
        kind: CapabilityKind::Tool,
        version: "1.0.0".into(),
        entry: "cap_signed".into(),
        status: CapabilityStatus::Registered,
        permission: forge_exec::PermissionLevel::ReadOnly,
    }).await.unwrap();
    // 用带 pool 的 state 重建 app（install 复验需要 PG）
    drop(app);
    let app = app_with_state(st_state);

    let (status, body) = send(
        app,
        post_json(
            "/market/install",
            serde_json::json!({"name": &name, "version": "1.0.0"}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "错签名 release 必须拒绝安装: {body}");
}

/// 冻结测试（MKT-102）：install 钉 yanked 版本 → 409；"*" 解析跳过 yanked → 404。
#[tokio::test]
async fn pinned_to_yanked_conflict_409() {
    let Some((_app, pool, tid)) = setup_app().await else { return; };

    let pub_y = format!("pub-y-{tid}");
    let name = format!("cap-yank-{tid}");
    let (_, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&pub_y)
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO releases (name, version, publisher_id, package_hash, signature, review_status, yanked) \
         VALUES ($1, '1.0.0', $2, 'hash-y', $3, 'published', true)",
    )
    .bind(&name)
    .bind(&pub_y)
    .bind("ab".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    let mut st = AppState::in_memory();
    st.pool = Some(pool);
    st.capabilities.register(Capability {
        id: forge_core::new_capability_id(),
        name: name.clone(),
        kind: CapabilityKind::Tool,
        version: "1.0.0".into(),
        entry: "cap_yank".into(),
        status: CapabilityStatus::Registered,
        permission: forge_exec::PermissionLevel::ReadOnly,
    }).await.unwrap();
    let app = app_with_state(st);

    // 钉 yanked → 409（显式优于静默）
    let (status, body) = send(
        app.clone(),
        post_json("/market/install", serde_json::json!({"name": &name, "version": "1.0.0"}), None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "钉 yanked 必须 409: {body}");

    // "*" 解析：唯一版本被 yanked 排除 → 无兼容版本
    let (status, _body) = send(
        app,
        post_json("/market/install", serde_json::json!({"name": &name, "version": "*"}), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "全 yanked 时 * 必须 404");
}
