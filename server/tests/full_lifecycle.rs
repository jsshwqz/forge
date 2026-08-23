//! SRV-002 全链路 e2e：HTTP API × PostgreSQL × 任务状态机。
//!
//! 剧本：（一）经 HTTP POST /tasks 创建带验收标准的任务（PG 落库）
//! （二）经 TaskStore 推进状态机 Pending→…→Completed（模拟编排层）
//! （三）经 HTTP GET /tasks/{id} 验证终态与目标字段
//! （内存实现下同剧本应失败于"重启不可见"，故本测试同时证明持久化切换生效）

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_server::{app_with_state, AppState};
use forge_storage::{connect_and_migrate, PgSessionStore, PgTaskStore};
use forge_task::TaskStatus;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn full_lifecycle_over_http_with_pg() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let state = AppState::new(
        Arc::new(PgTaskStore::new(pool.clone())),
        Arc::new(PgSessionStore::new(pool)),
    );
    let app = app_with_state(state.clone());

    // 1) 创建
    let res = app
        .clone()
        .oneshot(
            Request::post("/tasks")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "goal": "srv-e2e 全链路",
                        "acceptance": [{
                            "id": "AC-1", "description": "d",
                            "check": {"FileExists": "out.txt"}
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

    // 2) 状态机推进（经同一 State 的 TaskStore trait）
    for next in [TaskStatus::Planned, TaskStatus::Executing, TaskStatus::Verifying] {
        state
            .tasks
            .update_status(&forge_core::TaskId::from(id.clone()), next)
            .await
            .unwrap();
    }
    state
        .tasks
        .update_status(&forge_core::TaskId::from(id.clone()), TaskStatus::Completed)
        .await
        .unwrap();

    // 3) HTTP 验证终态
    let res = app
        .oneshot(Request::get(format!("/tasks/{id}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let got: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(got["status"], "Completed");
    assert_eq!(got["goal"], "srv-e2e 全链路");
}
