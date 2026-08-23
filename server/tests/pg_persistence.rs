//! INT-001 集成测试：HTTP API × PostgreSQL 持久化端到端。
//!
//! 证明：经 API 创建的任务真正落入数据库——用两个独立连接池模拟
//! 进程重启，第二个实例仍能读到（内存实现做不到）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::{app_with_state, AppState};
use forge_storage::{connect_and_migrate, PgSessionStore, PgTaskStore};
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
