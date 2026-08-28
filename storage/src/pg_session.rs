//! PgSessionStore：SessionStore trait 的 PostgreSQL 实现。
//!
//! 状态迁移规则与 forge-session 内存实现完全一致（复用 `Session::transition` 做校验，
//! kind→目标状态的映射在此镜像，见 `target_state_for` 文档注释）。
//! append 使用事务 + `SELECT ... FOR UPDATE` 行锁保证并发下 seq 连续且状态机安全。

use async_trait::async_trait;
use chrono::Utc;
use forge_core::{ForgeError, ForgeResult, SessionId, TaskId};
use forge_session::model::{Session, SessionEvent, SessionEventKind, SessionState};
use forge_session::SessionStore;
use sqlx::types::Json;
use sqlx::PgPool;

/// PostgreSQL 会话存储。
pub struct PgSessionStore {
    pool: PgPool,
}

impl PgSessionStore {
    /// 用现有连接池构造。
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// kind + 当前状态 → 目标状态（镜像 forge-session/src/store.rs 的同名逻辑；
/// 若核心规则变更，此处必须同步修改——两侧文档注释互为提醒）。
///
/// 部分支返回"故意非法"的目标状态：由 `Session::transition` 统一拒绝并报
/// InvalidState（与内存实现同一套校验路径）。
fn target_state_for(kind: &SessionEventKind, current: SessionState) -> Option<SessionState> {
    use SessionEventKind as K;
    use SessionState as S;
    match (current, kind) {
        (S::Active, K::Completed) => Some(S::Completed),
        (S::Active, K::Failed) => Some(S::Failed),
        (S::Active, K::Recovered) => Some(S::Recovering),
        (S::Active, _) => None,

        (S::Failed, K::Recovered) => Some(S::Recovering),
        (S::Failed, _) => Some(S::Active),

        (S::Recovering, K::Recovered) => Some(S::Recovering),
        (S::Recovering, K::Completed) => Some(S::Completed),
        (S::Recovering, K::Failed) => Some(S::Failed),
        (S::Recovering, _) => Some(S::Active),

        (S::Completed, _) => Some(S::Active),
    }
}

#[async_trait]
impl SessionStore for PgSessionStore {
    async fn create(&self, task_id: TaskId) -> ForgeResult<Session> {
        let id = SessionId::new_session_id();
        let mut tx = self.pool.begin().await.map_err(crate::db_err)?;
        sqlx::query("INSERT INTO sessions (id, task_id, state, tenant_id) VALUES ($1, $2, $3)")
            .bind(id.as_ref())
            .bind(task_id.as_ref())
            .bind(crate::enc(&SessionState::Active))
            .bind("default")
            .execute(&mut *tx)
            .await
            .map_err(crate::db_err)?;
        tx.commit().await.map_err(crate::db_err)?;
        Ok(Session { id, task_id, state: SessionState::Active, events: Vec::new() })
    }

    async fn append(
        &self,
        id: &SessionId,
        kind: SessionEventKind,
        payload: serde_json::Value,
    ) -> ForgeResult<SessionEvent> {
        let mut tx = self.pool.begin().await.map_err(crate::db_err)?;

        // 行锁读取当前状态
        let row: Option<(String,)> =
            sqlx::query_as("SELECT state FROM sessions WHERE id = $1 FOR UPDATE")
                .bind(id.as_ref())
                .bind("default")
                .fetch_optional(&mut *tx)
                .await
                .map_err(crate::db_err)?;
        let Some((state_s,)) = row else {
            return Err(ForgeError::NotFound(format!("session: {id}")));
        };
        let current: SessionState = crate::dec(&state_s)?;

        // 计算并校验迁移（复用 Core 的 transition 校验逻辑）
        let mut probe = Session {
            id: id.clone(),
            task_id: TaskId::from(String::new()),
            state: current,
            events: Vec::new(),
        };
        if let Some(target) = target_state_for(&kind, current) {
            probe.transition(target)?;
        }

        // 分配 seq 并落事件
        let seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_events WHERE session_id = $1",
        )
        .bind(id.as_ref())
        .fetch_one(&mut *tx)
        .await
        .map_err(crate::db_err)?;

        if let Some(target) = target_state_for(&kind, current) {
            sqlx::query("UPDATE sessions SET state = $2 WHERE id = $1")
                .bind(id.as_ref())
                .bind(crate::enc(&target))
                .execute(&mut *tx)
                .await
                .map_err(crate::db_err)?;
        }

        let at = Utc::now();
        sqlx::query(
            "INSERT INTO session_events (session_id, seq, at, kind, payload) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id.as_ref())
        .bind(seq)
        .bind(at)
        .bind(crate::enc(&kind))
        .bind(Json(payload.clone()))
        .execute(&mut *tx)
        .await
        .map_err(crate::db_err)?;

        tx.commit().await.map_err(crate::db_err)?;
        Ok(SessionEvent { seq: seq as u64, at, kind, payload })
    }

    async fn get(&self, id: &SessionId) -> ForgeResult<Session> {
        let s_row: Option<(String, String)> =
            sqlx::query_as("SELECT task_id, state FROM sessions WHERE id = $1")
                .bind(id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        let Some((task_s, state_s)) = s_row else {
            return Err(ForgeError::NotFound(format!("session: {id}")));
        };

        let rows: Vec<(i64, chrono::DateTime<chrono::Utc>, String, Json<serde_json::Value>)> =
            sqlx::query_as(
                "SELECT seq, at, kind, payload FROM session_events WHERE session_id = $1 ORDER BY seq",
            )
            .bind(id.as_ref())
            .fetch_all(&self.pool)
            .await
            .map_err(crate::db_err)?;

        let mut events = Vec::with_capacity(rows.len());
        for (seq, at, kind_s, Json(payload)) in rows {
            events.push(SessionEvent {
                seq: seq as u64,
                at,
                kind: crate::dec(&kind_s)?,
                payload,
            });
        }

        Ok(Session {
            id: id.clone(),
            task_id: TaskId::from(task_s),
            state: crate::dec(&state_s)?,
            events,
        })
    }

    async fn list(&self) -> ForgeResult<Vec<SessionId>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM sessions ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(crate::db_err)?;
        Ok(rows.into_iter().map(|(s,)| SessionId::from(s)).collect())
    }
}
