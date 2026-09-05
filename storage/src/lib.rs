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
pub mod pg_task;
pub mod s3;

pub use pg_artifact::PgArtifactStore;
pub use pg_evidence::PgEvidenceStore;
pub use pg_session::PgSessionStore;
pub use pg_task::PgTaskStore;
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

/// 连接池上下限解析（DEP-001 D1：`FORGE_DB_MAX_CONN`/`FORGE_DB_MIN_CONN`）。
///
/// 纯函数便于离线测试；默认 (10, 2) 与 docs/SCALING.md D1 表一致。
/// max ≤ 0 或解析失败回退默认；min 越界（≥max 或 <0）回退 2。
pub(crate) fn parse_pool_bounds(max: Option<&str>, min: Option<&str>) -> (u32, u32) {
    let def: (u32, u32) = (10, 2);
    let max_v = max
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(def.0);
    let min_v = min
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| *n < max_v)
        .unwrap_or(def.1.min(max_v));
    (max_v, min_v)
}

/// 读取连接池上下限：env `FORGE_DB_MAX_CONN` / `FORGE_DB_MIN_CONN`（DEP-001 D1）。
pub fn pool_bounds_from_env() -> (u32, u32) {
    parse_pool_bounds(
        std::env::var("FORGE_DB_MAX_CONN").ok().as_deref(),
        std::env::var("FORGE_DB_MIN_CONN").ok().as_deref(),
    )
}

/// 建池并执行幂等 DDL 迁移。
pub async fn connect_and_migrate(url: &str) -> ForgeResult<PgPool> {
    let (max_conn, min_conn) = pool_bounds_from_env();
    let pool = PgPoolOptions::new()
        .max_connections(max_conn)
        .min_connections(min_conn)
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

CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    goal        TEXT NOT NULL,
    constraints JSONB NOT NULL DEFAULT '[]'::jsonb,
    acceptance  JSONB NOT NULL DEFAULT '[]'::jsonb,
    status      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
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

-- V5.0 TEN-001/002/003（storage/migrations/0009~0011 的内嵌化，R7-010）：
-- 0009 仅对 PG 实存的 tasks/sessions 加列；product_instances/templates 为内存
-- MVP（WORKLOG R7-008），其 PG ALTER 留待对应存储实现立项时随迁移文件执行。
CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY DEFAULT 'default',
    name TEXT NOT NULL DEFAULT 'Default Tenant',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
ALTER TABLE tasks ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';
CREATE INDEX IF NOT EXISTS idx_tasks_tenant_id ON tasks(tenant_id);
CREATE INDEX IF NOT EXISTS idx_sessions_tenant_id ON sessions(tenant_id);
INSERT INTO tenants (id, name) VALUES ('default', 'Default Tenant')
ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS tenant_keys (
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    key_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, key_hash)
);

CREATE TABLE IF NOT EXISTS quotas (
    tenant_id TEXT PRIMARY KEY REFERENCES tenants(id),
    max_concurrent INT NOT NULL DEFAULT 4,
    daily_tasks INT NOT NULL DEFAULT 100
);
INSERT INTO quotas (tenant_id, max_concurrent, daily_tasks)
VALUES ('default', 4, 100)
ON CONFLICT (tenant_id) DO NOTHING;

-- FED-001：多副本任务队列（storage/migrations/0012 同款，内嵌保证真实应用——R7-010）
CREATE TABLE IF NOT EXISTS task_queue (
    id           BIGSERIAL PRIMARY KEY,
    task_id      TEXT NOT NULL,
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    payload      JSONB NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending',
    claimed_by   TEXT,
    claimed_at   TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_task_queue_claim
    ON task_queue(status, lease_expires_at) WHERE status IN ('pending','claimed');

-- MKT-101：发布者生态与签名（storage/migrations/0013 同款，内嵌保证真实应用）
CREATE TABLE IF NOT EXISTS publisher_keys (
    publisher_id TEXT PRIMARY KEY,
    public_key   TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS releases (
    id           BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    version      TEXT NOT NULL,
    publisher_id TEXT NOT NULL REFERENCES publisher_keys(publisher_id),
    package_hash TEXT NOT NULL,
    signature    TEXT NOT NULL,
    review_status TEXT NOT NULL DEFAULT 'pending',
    UNIQUE(name, version, publisher_id)
);
"#;

#[cfg(test)]
mod pool_bounds_tests {
    use super::parse_pool_bounds;

    #[test]
    fn defaults_when_unset() {
        assert_eq!(parse_pool_bounds(None, None), (10, 2));
        assert_eq!(parse_pool_bounds(Some(""), Some("")), (10, 2));
    }

    #[test]
    fn env_values_win() {
        assert_eq!(parse_pool_bounds(Some("20"), Some("5")), (20, 5));
        assert_eq!(parse_pool_bounds(Some(" 30 "), None), (30, 2));
    }

    #[test]
    fn invalid_falls_back() {
        assert_eq!(parse_pool_bounds(Some("abc"), Some("x")), (10, 2));
        assert_eq!(parse_pool_bounds(Some("0"), None), (10, 2));
    }

    #[test]
    fn min_clamped_below_max() {
        assert_eq!(parse_pool_bounds(Some("2"), Some("9")), (2, 2));
    }
}
