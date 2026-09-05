//! V6.0 FED-001：多副本任务队列（build_v60.md AF-BP-V60A 契约）。
//!
//! 竞争消费：多副本 worker 抢认领同一队列，`FOR UPDATE SKIP LOCKED` 保证
//! 同一行仅一个 worker 拿到（R1：单条 SQL 原子完成，禁止"先查后改"两步）。
//! 租约到期未被 complete 的行可被重认领（worker 崩溃自愈）。

use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};

/// 队列事件频道名（R3：冻结）。
pub const FORGE_EVENTS_CHANNEL: &str = "forge_events";

/// 认领出的队列任务。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedTask {
    pub id: i64,
    pub task_id: String,
    pub tenant_id: String,
    pub payload: serde_json::Value,
}

/// 本副本 worker 标识（R2：`wrk_<uuid>`，进程生命周期内不变）。
pub fn new_worker_id() -> String {
    format!("wrk_{}", uuid::Uuid::new_v4())
}

/// 竞争认领：pending 或租约过期的 claimed 行中取最旧一条并原子置为 claimed。
/// 返回 `None` 表示当前无可认领任务。
pub async fn claim_next_task(
    pool: &sqlx::PgPool,
    worker_id: &str,
    lease_secs: i64,
) -> ForgeResult<Option<QueuedTask>> {
    let row: Option<(i64, String, String, serde_json::Value)> = sqlx::query_as(
        "UPDATE task_queue SET status = 'claimed', claimed_by = $1, claimed_at = now(), \
         lease_expires_at = now() + make_interval(secs => $2) \
         WHERE id = ( \
           SELECT id FROM task_queue \
           WHERE status = 'pending' OR (status = 'claimed' AND lease_expires_at < now()) \
           ORDER BY id LIMIT 1 \
           FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING id, task_id, tenant_id, payload",
    )
    .bind(worker_id)
    .bind(lease_secs)
    .fetch_optional(pool)
    .await
    .map_err(|e| ForgeError::InvalidState(format!("queue claim: {e}")))?;

    Ok(row.map(|(id, task_id, tenant_id, payload)| QueuedTask {
        id,
        task_id,
        tenant_id,
        payload,
    }))
}

/// 标记队列行完成/失败（仅本 worker 持有的行）。
pub async fn complete_task(pool: &sqlx::PgPool, id: i64, ok: bool) -> ForgeResult<()> {
    let status = if ok { "done" } else { "failed" };
    let r = sqlx::query("UPDATE task_queue SET status = $2 WHERE id = $1 AND status = 'claimed'")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .map_err(|e| ForgeError::InvalidState(format!("queue complete: {e}")))?;
    if r.rows_affected() == 0 {
        return Err(ForgeError::NotFound(format!("claimed queue row: {id}")));
    }
    Ok(())
}

/// 启动时清理：把租约已过期的 claimed 行重置回 pending，返回重置行数。
pub async fn reap_expired_leases(pool: &sqlx::PgPool) -> ForgeResult<u64> {
    let r = sqlx::query(
        "UPDATE task_queue SET status = 'pending', claimed_by = NULL, claimed_at = NULL, \
         lease_expires_at = NULL WHERE status = 'claimed' AND lease_expires_at < now()",
    )
    .execute(pool)
    .await
    .map_err(|e| ForgeError::InvalidState(format!("queue reap: {e}")))?;
    Ok(r.rows_affected())
}

/// 编排入队：为已建任务投递一条队列记录（R4 路径的第一步）。
pub async fn enqueue_orchestration(
    pool: &sqlx::PgPool,
    task_id: &str,
    tenant_id: &str,
    payload: serde_json::Value,
) -> ForgeResult<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO task_queue (task_id, tenant_id, payload) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(task_id)
    .bind(tenant_id)
    .bind(payload)
    .fetch_one(pool)
    .await
    .map_err(|e| ForgeError::InvalidState(format!("queue enqueue: {e}")))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_id_format() {
        let wid = new_worker_id();
        assert!(wid.starts_with("wrk_"), "R2: worker_id 必须 wrk_ 前缀: {wid}");
    }

    #[test]
    fn queued_task_roundtrip() {
        let qt = QueuedTask {
            id: 1,
            task_id: "task_x".into(),
            tenant_id: "default".into(),
            payload: serde_json::json!({"timeout_secs": 30}),
        };
        let s = serde_json::to_string(&qt).unwrap();
        let back: QueuedTask = serde_json::from_str(&s).unwrap();
        assert_eq!(back.task_id, "task_x");
    }
}
