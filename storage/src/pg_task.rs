//! PgTaskStore：TaskStore trait 的 PostgreSQL 实现。
//! 状态迁移复用 forge_task::Task::transition 校验（与内存实现同一路径）。
//! update_status 使用事务 + FOR UPDATE 防并发竞态。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_task::{AcceptanceCriterion, Task, TaskStatus, TaskStore};
use sqlx::types::Json;
use sqlx::PgPool;

/// PostgreSQL 任务存储。
pub struct PgTaskStore {
    pool: PgPool,
}

impl PgTaskStore {
    /// 用现有连接池构造。
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// update_status 的加锁查询。
    ///
    /// 契约（见本文件末尾回归测试）：
    /// - `SELECT` 列表列数必须等于 `update_status` 行元组的字段数（5）；
    /// - `WHERE` 只允许出现一次；
    /// - 只绑 `$1`（任务 id）。
    const UPDATE_STATUS_SELECT: &str =
        "SELECT goal, constraints, acceptance, status, created_at FROM tasks WHERE id = $1 FOR UPDATE";
}

#[async_trait]
impl TaskStore for PgTaskStore {
    async fn create(
        &self,
        goal: String,
        constraints: Vec<String>,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> ForgeResult<Task> {
        let id = TaskId::new_task_id();
        let created_at = Utc::now();
        sqlx::query("INSERT INTO tasks (id, goal, constraints, acceptance, status, tenant_id, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(id.as_ref())
            .bind(&goal)
            .bind(Json(constraints.clone()))
            .bind(Json(acceptance.clone()))
            .bind(crate::enc(&TaskStatus::Pending))
            .bind("default")
            .bind(created_at)
            .execute(&self.pool)
            .await
            .map_err(crate::db_err)?;

        Ok(Task { id, goal, constraints, acceptance, status: TaskStatus::Pending, created_at })
    }

    async fn get(&self, id: &TaskId) -> ForgeResult<Task> {
        let row: Option<(String, Json<Vec<String>>, Json<Vec<AcceptanceCriterion>>, String, String, DateTime<Utc>)> =
            sqlx::query_as("SELECT goal, constraints, acceptance, status, tenant_id, created_at FROM tasks WHERE id = $1")
                .bind(id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        let Some((goal, Json(constraints), Json(acceptance), status_s, _tenant_id, created_at)) = row else {
            return Err(ForgeError::NotFound(format!("task: {id}")));
        };
        Ok(Task {
            id: id.clone(),
            goal,
            constraints,
            acceptance,
            status: crate::dec(&status_s)?,
            created_at,
        })
    }

    async fn update_status(&self, id: &TaskId, to: TaskStatus) -> ForgeResult<Task> {
        let mut tx = self.pool.begin().await.map_err(crate::db_err)?;
        let row: Option<(String, Json<Vec<String>>, Json<Vec<AcceptanceCriterion>>, String, DateTime<Utc>)> =
            sqlx::query_as(Self::UPDATE_STATUS_SELECT)
                .bind(id.as_ref())
                .fetch_optional(&mut *tx)
                .await
                .map_err(crate::db_err)?;
        let Some((goal, Json(constraints), Json(acceptance), status_s, created_at)) = row else {
            return Err(ForgeError::NotFound(format!("task: {id}")));
        };

        // 复用 Core 的状态机校验（含空验收禁令）
        let mut task = Task {
            id: id.clone(),
            goal,
            constraints,
            acceptance,
            status: crate::dec(&status_s)?,
            created_at,
        };
        task.transition(to)?;

        sqlx::query("UPDATE tasks SET status = $2 WHERE id = $1")
            .bind(id.as_ref())
            .bind(crate::enc(&to))
            .execute(&mut *tx)
            .await
            .map_err(crate::db_err)?;
        tx.commit().await.map_err(crate::db_err)?;

        task.status = to;
        Ok(task)
    }

    async fn list(&self) -> ForgeResult<Vec<TaskId>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM tasks ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(crate::db_err)?;
        Ok(rows.into_iter().map(|(s,)| TaskId::from(s)).collect())
    }

    /// 租户域内取任务：`WHERE id = $1 AND tenant_id = $2`（V5-FIX-2d）。
    /// 任务存在但属其它租户 → `PermissionDenied("cross-tenant access blocked")`。
    async fn get_in_tenant(&self, tenant_id: &str, id: &TaskId) -> ForgeResult<Task> {
        let row: Option<(String, Json<Vec<String>>, Json<Vec<AcceptanceCriterion>>, String, DateTime<Utc>)> =
            sqlx::query_as(
                "SELECT goal, constraints, acceptance, status, created_at FROM tasks WHERE id = $1 AND tenant_id = $2",
            )
            .bind(id.as_ref())
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(crate::db_err)?;
        let Some((goal, Json(constraints), Json(acceptance), status_s, created_at)) = row else {
            let exists: Option<(String,)> =
                sqlx::query_as("SELECT tenant_id FROM tasks WHERE id = $1")
                    .bind(id.as_ref())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(crate::db_err)?;
            if exists.is_some() {
                return Err(ForgeError::PermissionDenied(
                    "cross-tenant access blocked".into(),
                ));
            }
            return Err(ForgeError::NotFound(format!("task: {id}")));
        };
        Ok(Task {
            id: id.clone(),
            goal,
            constraints,
            acceptance,
            status: crate::dec(&status_s)?,
            created_at,
        })
    }

    /// 租户域内列举：`WHERE tenant_id = $1`（V5-FIX-2d，走 idx_tasks_tenant_id）。
    async fn list_in_tenant(&self, tenant_id: &str) -> ForgeResult<Vec<TaskId>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM tasks WHERE tenant_id = $1 ORDER BY id")
                .bind(tenant_id)
                .fetch_all(&self.pool)
                .await
                .map_err(crate::db_err)?;
        Ok(rows.into_iter().map(|(s,)| TaskId::from(s)).collect())
    }

    /// 租户域内进行中任务数（TEN-003 R3：走 tenant_id 索引，禁全表扫）。
    /// status 列存 JSON 编码（如 `"Completed"` 含引号）。
    async fn count_running(&self, tenant_id: &str) -> ForgeResult<i64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks WHERE tenant_id = $1 AND status NOT IN ('\"Completed\"', '\"Failed\"')",
        )
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
        .map_err(crate::db_err)?;
        Ok(n)
    }

    /// 租户域内当日创建任务数（TEN-003 R3：走 tenant_id 索引）。
    async fn count_today(&self, tenant_id: &str) -> ForgeResult<i64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks WHERE tenant_id = $1 AND created_at >= date_trunc('day', now())",
        )
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
        .map_err(crate::db_err)?;
        Ok(n)
    }
}
