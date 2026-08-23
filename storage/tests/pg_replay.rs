//! 持久化 × 回放 兼容性验证：
//! PgSessionStore 写入的事件序列必须能被 forge_session::replay 确定性重建，
//! 且与库内读回状态一致（M1 replay 承诺在持久化下的延续）。

use forge_session::{replay, SessionEventKind, SessionState, SessionStore};
use forge_storage::{connect_and_migrate, PgSessionStore};

#[tokio::test]
async fn pg_events_are_replayable() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let store = PgSessionStore::new(pool);

    let s = store.create(forge_core::TaskId::new_task_id()).await.unwrap();

    // 正常完成路径
    for kind in [
        SessionEventKind::TaskReceived,
        SessionEventKind::PlanCreated,
        SessionEventKind::ActionDispatched,
        SessionEventKind::ActionResult,
        SessionEventKind::Completed,
    ] {
        store.append(&s.id, kind.clone(), serde_json::json!({})).await.unwrap();
    }

    let from_db = store.get(&s.id).await.unwrap();

    // ① 库内状态 == replay(事件序列)
    let replayed = replay(&from_db.events).unwrap();
    assert_eq!(replayed, from_db.state);
    assert_eq!(replayed, SessionState::Completed);

    // ② 失败→恢复 路径同样成立
    let s2 = store.create(forge_core::TaskId::new_task_id()).await.unwrap();
    store.append(&s2.id, SessionEventKind::Failed, serde_json::json!({})).await.unwrap();
    store.append(&s2.id, SessionEventKind::Recovered, serde_json::json!({})).await.unwrap();
    store
        .append(&s2.id, SessionEventKind::ActionDispatched, serde_json::json!({}))
        .await
        .unwrap();
    let db2 = store.get(&s2.id).await.unwrap();
    assert_eq!(replay(&db2.events).unwrap(), db2.state);
    assert_eq!(db2.state, SessionState::Active);

    // ③ 确定性：同一事件序列两次回放结果一致
    assert_eq!(replay(&db2.events).unwrap(), replay(&db2.events).unwrap());
}
