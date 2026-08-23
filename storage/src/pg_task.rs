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
        sqlx::query("INSERT INTO tasks (id, goal, constraints, acceptance, status, created_at) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(id.as_ref())
            .bind(&goal)
            .bind(Json(constraints.clone()))
            .bind(Json(acceptance.clone()))
            .bind(crate::enc(&TaskStatus::Pending))
            .bind(created_at)
            .execute(&self.pool)
            .await
            .map_err(crate::db_err)?;

        Ok(Task { id, goal, constraints, acceptance, status: TaskStatus::Pending, created_at })
    }

    async fn get(&self, id: &TaskId) -> ForgeResult<Task> {
        let row: Option<(String, Json<Vec<String>>, Json<Vec<AcceptanceCriterion>>, String, DateTime<Utc>)> =
            sqlx::query_as("SELECT goal, constraints, acceptance, status, created_at FROM tasks WHERE id = $1")
                .bind(id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        let Some((goal, Json(constraints), Json(acceptance), status_s, created_at)) = row else {
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
            sqlx::query_as("SELECT goal, constraints, acceptance, status, created_at FROM tasks WHERE id = $1 FOR UPDATE")
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
}
