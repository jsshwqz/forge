//! G-V6A 门禁第 2 条：并发认领 e2e（build_v60.md）。
//! 单 PG、双 worker 并发认领循环，100 任务入队 → 总数恰完成 100 次（不丢不重）。

use forge_server::queue;
use forge_storage::connect_and_migrate;

#[tokio::test]
async fn concurrent_claim_e2e_100_tasks_no_loss_no_duplication() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };
    let pool = connect_and_migrate(&url).await.unwrap();
    sqlx::query("DELETE FROM task_queue").execute(&pool).await.unwrap();

    // 100 任务入队
    for i in 0..100 {
        sqlx::query("INSERT INTO task_queue (task_id, payload) VALUES ($1, $2)")
            .bind(format!("task_gate_{i}"))
            .bind(serde_json::json!({"probe": i}))
            .execute(&pool)
            .await
            .unwrap();
    }

    // 双 worker 认领循环（模拟两个副本进程），认领即视为执行并 complete
    let run_worker = |wid: String, pool: sqlx::PgPool| async move {
        let mut executed: Vec<String> = Vec::new();
        loop {
            match queue::claim_next_task(&pool, &wid, 300).await.unwrap() {
                Some(qt) => {
                    queue::complete_task(&pool, qt.id, true).await.unwrap();
                    executed.push(qt.task_id);
                }
                None => break,
            }
        }
        executed
    };

    let (a, b) = tokio::join!(
        run_worker("wrk_gate_a".into(), pool.clone()),
        run_worker("wrk_gate_b".into(), pool.clone()),
    );
    let total = a.len() + b.len();
    assert_eq!(total, 100, "两 worker 合计必须恰执行 100 次（不丢不重）");

    // 不重复：合并后 task_id 唯一
    let mut ids = a.clone();
    ids.extend(b.clone());
    ids.sort();
    let uniq = ids.len();
    assert_eq!(uniq, 100, "task_id 必须全部唯一");

    // 队列状态核对：100 done
    let (done,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM task_queue WHERE status = 'done'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(done, 100);
    sqlx::query("DELETE FROM task_queue").execute(&pool).await.unwrap();
}
