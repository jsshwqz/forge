//! INT-001 集成测试：HTTP API × PostgreSQL 持久化端到端。
//!
//! 证明：经 API 创建的任务真正落入数据库——用两个独立连接池模拟
//! 进程重启，第二个实例仍能读到（内存实现做不到）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_core::ForgeError;
use forge_server::{app_with_state, AppState};
use forge_storage::{connect_and_migrate, PgSessionStore, PgTaskStore};
use forge_task::TaskStore as _;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn task_survives_storage_restart() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };

    // —— 实例 A：经 HTTP 创建任务 ——
    let pool_a = connect_and_migrate(&url).await.unwrap();
    let state_a = AppState::new(
        Arc::new(PgTaskStore::new(pool_a.clone())),
        Arc::new(PgSessionStore::new(pool_a)),
    );
    let res = app_with_state(state_a)
        .oneshot(
            Request::post("/tasks")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "goal": "persist-across-restart",
                        "acceptance": [{
                            "id": "AC-1",
                            "description": "d",
                            "check": {"FileExists": "o.txt"}
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    // pool_a 在此 drop —— 模拟进程退出

    // —— 实例 B：全新连接池 = "重启后" ——
    let pool_b = connect_and_migrate(&url).await.unwrap();
    let state_b = AppState::new(
        Arc::new(PgTaskStore::new(pool_b.clone())),
        Arc::new(PgSessionStore::new(pool_b)),
    );
    let res = app_with_state(state_b)
        .oneshot(Request::get(format!("/tasks/{id}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "重启后任务必须可读");
    let got: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(got["goal"], "persist-across-restart");
}

/// V5-FIX-2d（TEN-001）冻结测试：租户域列举只返回本租户任务。
#[tokio::test]
async fn tenant_isolation_list() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let store = PgTaskStore::new(pool);

    let t1 = store.create("tenant-list-1".into(), vec![], vec![]).await.unwrap();
    let t2 = store.create("tenant-list-2".into(), vec![], vec![]).await.unwrap();

    let in_default = store.list_in_tenant("default").await.unwrap();
    assert!(in_default.contains(&t1.id), "default 租户必须看到自己的任务");
    assert!(in_default.contains(&t2.id));

    let in_ghost = store.list_in_tenant("ghost-tenant").await.unwrap();
    assert!(in_ghost.is_empty(), "其它租户列举必须为空");
}

/// V5-FIX-2d（TEN-001）冻结测试：跨租户读取必须 PermissionDenied。
#[tokio::test]
async fn cross_tenant_get_blocked() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let store = PgTaskStore::new(pool);

    let t = store.create("cross-tenant-probe".into(), vec![], vec![]).await.unwrap();

    let err = store
        .get_in_tenant("other-tenant", &t.id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ForgeError::PermissionDenied(ref m) if m.contains("cross-tenant access blocked")),
        "跨租户读取必须 PermissionDenied: {err}"
    );

    // 同租户可读
    let ok = store.get_in_tenant("default", &t.id).await.unwrap();
    assert_eq!(ok.id, t.id);
}
