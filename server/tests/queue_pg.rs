//! V6.0 FED-001：多副本任务队列 + PG 事件总线（build_v60.md 冻结测试，PG 门控）。
//!
//! 冻结测试名：
//! - claim_is_exclusive_under_concurrency（10 并发认领 1 条，恰 1 成功）
//! - expired_lease_reclaimed（过期租约 reap 后回 pending）
//! - notify_received_by_listener（listen 后 notify，receiver 收原文）
//! - worker_crash_task_recovers（租约过期后另一 worker 认领成功）

use forge_core::ForgeResult;
use forge_server::bus::PgBus;
use forge_server::queue;
use forge_storage::connect_and_migrate;

/// 队列表是共享的：四个测试互斥执行，避免互相清空对方的行。
static QUEUE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> Option<(sqlx::PgPool, tokio::sync::MutexGuard<'static, ()>)> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    let guard = QUEUE_LOCK.lock().await;
    // 测试隔离：专用测试库口径，清空队列残留（此前失败运行的遗留行会干扰认领计数）
    sqlx::query("DELETE FROM task_queue").execute(&pool).await.unwrap();
    Some((pool, guard))
}

/// 冻结测试：10 并发认领 1 条任务，恰 1 个成功。
#[tokio::test]
async fn claim_is_exclusive_under_concurrency() {
    let Some((pool, _guard)) = pool().await else { return; };
    let (qid,): (i64,) = sqlx::query_as(
        "INSERT INTO task_queue (task_id, payload) VALUES ('task_exclusive', '{}') RETURNING id",
    )
    .bind("task_exclusive")
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut handles = Vec::new();
    for i in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            queue::claim_next_task(&pool, &format!("wrk_probe_{i}"), 300).await
        }));
    }
    let mut winners = 0;
    for h in handles {
        if let Ok(Ok(Some(qt))) = h.await {
            assert_eq!(qt.id, qid);
            winners += 1;
        }
    }
    assert_eq!(winners, 1, "10 并发认领 1 条必须恰 1 成功");
    // 清理
    let _ = sqlx::query("DELETE FROM task_queue WHERE id = $1").bind(qid).execute(&pool).await;
}

/// 冻结测试：塞入过期 claimed 行，reap 后状态回 pending 并可被认领。
#[tokio::test]
async fn expired_lease_reclaimed() {
    let Some((pool, _guard)) = pool().await else { return; };
    // 直接插入一条已过期租约的 claimed 行
    let (qid,): (i64,) = sqlx::query_as(
        "INSERT INTO task_queue (task_id, payload, status, claimed_by, claimed_at, lease_expires_at) \
         VALUES ('task_stale', '{}', 'claimed', 'wrk_dead', now(), now() - interval '1 hour') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let reaped = queue::reap_expired_leases(&pool).await.unwrap();
    assert!(reaped >= 1, "过期租约必须被 reap");

    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM task_queue WHERE id = $1").bind(qid).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "pending", "reap 后必须回 pending");

    // 回 pending 后可被新 worker 认领
    let claimed = queue::claim_next_task(&pool, "wrk_fresh", 300).await.unwrap();
    assert!(claimed.is_some(), "pending 行必须可被认领");
    let _ = sqlx::query("DELETE FROM task_queue WHERE id = $1").bind(qid).execute(&pool).await;
}

/// 冻结测试：listen 后 notify，receiver 收到原文。
#[tokio::test]
async fn notify_received_by_listener() {
    let Some((pool, _guard)) = pool().await else { return; };
    let mut rx = PgBus::listen(&pool, queue::FORGE_EVENTS_CHANNEL).await.unwrap();
    // NOTIFY 是同事务/连接可见性：给监听建立留一点时间
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    PgBus::notify(&pool, queue::FORGE_EVENTS_CHANNEL, r#"{"probe":"fed001"}"#)
        .await
        .unwrap();
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内必须收到通知")
        .unwrap();
    assert_eq!(msg, r#"{"probe":"fed001"}"#, "必须收到原文");
}

/// 冻结测试：模拟 worker 崩溃（租约过期）后另一 worker 认领成功。
#[tokio::test]
async fn worker_crash_task_recovers() {
    let Some((pool, _guard)) = pool().await else { return; };
    let (qid,): (i64,) = sqlx::query_as(
        "INSERT INTO task_queue (task_id, payload) VALUES ('task_crash', '{}') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // worker A 以 1 秒租约认领后"崩溃"（不再 complete）
    let claimed_a = queue::claim_next_task(&pool, "wrk_crash_a", 1).await.unwrap();
    assert!(claimed_a.is_some(), "worker A 首次认领必须成功");

    // 等租约过期
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    // worker B 认领：无需 reap 也应能直接拿走过期行
    let claimed_b = queue::claim_next_task(&pool, "wrk_crash_b", 300).await.unwrap();
    assert!(claimed_b.is_some(), "租约过期后另一 worker 必须能认领");
    assert_eq!(claimed_b.unwrap().id, qid);

    // 完成后不可再认领
    let _ = queue::complete_task(&pool, qid, true).await;
    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM task_queue WHERE id = $1").bind(qid).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "done");
    let _ = sqlx::query("DELETE FROM task_queue WHERE id = $1").bind(qid).execute(&pool).await;
}

/// 静态约束：ForgeResult 仍为统一错误类型。
#[allow(dead_code)]
fn _result_alias_check() -> ForgeResult<()> {
    Ok(())
}
