//! forge-storage：第二阶段持久化实现（PH2-001，施工包 B-01 / 技术栈冻结 PostgreSQL+sqlx）。
//!
//! 以 PostgreSQL 实现第一阶段预留的三个存储 trait：
//! - [`pg_session::PgSessionStore`]   ← forge_session::SessionStore
//! - [`pg_artifact::PgArtifactStore`] ← forge_artifact::ArtifactStore
//! - [`pg_evidence::PgEvidenceStore`] ← forge_evidence::EvidenceStore
//!
//! 架构决策：独立 crate 承载 sqlx 重依赖，Core 各 crate 保持零存储依赖
//! （AP-003 分离精神；Server/装配层按需替换 State 组装即可，见 PH2-002）。
//!
//! 错误映射：sqlx::Error 统一包为 `ForgeError::InvalidState("db error: ...")`，
//! 不修改 Core 的错误枚举（避免第一阶段冻结面变化）。
//!
//! 集成测试：设置 `FORGE_PG_URL` 后运行（见 tests/pg.rs）；
//! 未设置时测试自动跳过并打印说明（DoD 要求必须设置后跑绿）。

pub mod pg_artifact;
pub mod pg_evidence;
pub mod pg_session;
pub mod s3;

pub use pg_artifact::PgArtifactStore;
pub use pg_evidence::PgEvidenceStore;
pub use pg_session::PgSessionStore;
pub use s3::{MinioArtifactStore, S3Config};

use forge_core::{ForgeError, ForgeResult};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

/// 默认连接串（本机 Podman forge-pg 容器）。
/// 注意：WSL2 端口转发仅绑定 ::1，必须用 `localhost` 而非 `127.0.0.1`。
pub const DEFAULT_DATABASE_URL: &str = "postgres://postgres:forge@localhost:15432/forge";

/// 读取连接串：环境变量 `FORGE_PG_URL` 优先，否则默认值。
pub fn database_url() -> String {
    std::env::var("FORGE_PG_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string())
}

/// sqlx 错误 → ForgeError 统一包装。
pub(crate) fn db_err(e: sqlx::Error) -> ForgeError {
    ForgeError::InvalidState(format!("db error: {e}"))
}

/// 枚举/类型编码为 JSON 字符串（serde 单元变体 → `"Variant"`）。
pub(crate) fn enc<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("forge-storage: encode cannot fail for unit enums")
}

/// 从 JSON 字符串解码。
pub(crate) fn dec<T: DeserializeOwned>(s: &str) -> ForgeResult<T> {
    serde_json::from_str(s).map_err(|e| ForgeError::InvalidState(format!("decode failed: {e}")))
}

/// 建池并执行幂等 DDL 迁移。
pub async fn connect_and_migrate(url: &str) -> ForgeResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
        .map_err(|e| ForgeError::InvalidState(format!("pg connect failed: {e}")))?;
    migrate(&pool).await?;
    Ok(pool)
}

/// 进程内迁移去重锁（多测试并行建池时只跑一次）。
static MIGRATE_LOCK: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// 幂等建表。
///
/// 双重串行化：
/// 1. 进程内 `Mutex`——并行测试/多次调用只执行一次；
/// 2. `pg_advisory_xact_lock`——跨进程（如 server 与测试同时启动）也互斥，
///    规避 PostgreSQL `CREATE TABLE IF NOT EXISTS` 在并发下的 pg_type 竞态。
pub async fn migrate(pool: &PgPool) -> ForgeResult<()> {
    let mut done = MIGRATE_LOCK.lock().await;
    if !*done {
        let mut tx = pool.begin().await.map_err(db_err)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('forge_storage_migrations'))")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::raw_sql(MIGRATIONS)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        *done = true;
    }
    Ok(())
}

const MIGRATIONS: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL,
    state      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS session_events (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    seq        BIGINT NOT NULL,
    at         TIMESTAMPTZ NOT NULL,
    kind       TEXT NOT NULL,
    payload    JSONB NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE IF NOT EXISTS artifacts (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL,
    meta            JSONB NOT NULL DEFAULT '{}'::jsonb,
    content         BYTEA NOT NULL
);

CREATE TABLE IF NOT EXISTS evidence (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    criterion_id TEXT NOT NULL,
    content      TEXT NOT NULL,
    produced_by  TEXT NOT NULL,
    at           TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_evidence_criterion ON evidence(criterion_id);
"#;
