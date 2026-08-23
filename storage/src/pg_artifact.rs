//! PgArtifactStore：ArtifactStore trait 的 PostgreSQL 实现。
//!
//! 内容以 BYTEA 存储（第一阶段语义等价迁移）；MinIO(S3) 对象存储为后续
//! 独立小任务（PH2-001b），trait 不变仅换实现。
//! checksum 沿用 forge-artifact 的算法约定：SHA-256 小写十六进制。

use async_trait::async_trait;
use chrono::Utc;
use forge_artifact::{Artifact, ArtifactKind, ArtifactStore};
use forge_core::{ArtifactId, ForgeError, ForgeResult};
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use sqlx::PgPool;

/// PostgreSQL 产物存储。
pub struct PgArtifactStore {
    pool: PgPool,
}

impl PgArtifactStore {
    /// 用现有连接池构造。
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[async_trait]
impl ArtifactStore for PgArtifactStore {
    async fn put(
        &self,
        name: String,
        kind: ArtifactKind,
        content: Vec<u8>,
        meta: serde_json::Value,
    ) -> ForgeResult<Artifact> {
        let id = ArtifactId::new_artifact_id();
        let checksum = sha256_hex(&content);
        let size = content.len() as i64;
        let created_at = Utc::now();
        sqlx::query(
            "INSERT INTO artifacts (id, name, kind, checksum_sha256, size_bytes, created_at, meta, content)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id.as_ref())
        .bind(&name)
        .bind(crate::enc(&kind))
        .bind(&checksum)
        .bind(size)
        .bind(created_at)
        .bind(Json(meta.clone()))
        .bind(&content)
        .execute(&self.pool)
        .await
        .map_err(crate::db_err)?;

        Ok(Artifact {
            id,
            name,
            kind,
            checksum_sha256: checksum,
            size_bytes: size as u64,
            created_at,
            meta,
        })
    }

    async fn get_meta(&self, id: &ArtifactId) -> ForgeResult<Artifact> {
        let row: Option<(String, String, String, i64, chrono::DateTime<chrono::Utc>, Json<serde_json::Value>)> =
            sqlx::query_as(
                "SELECT name, kind, checksum_sha256, size_bytes, created_at, meta FROM artifacts WHERE id = $1",
            )
            .bind(id.as_ref())
            .fetch_optional(&self.pool)
            .await
            .map_err(crate::db_err)?;
        let Some((name, kind_s, checksum, size, created_at, Json(meta))) = row else {
            return Err(ForgeError::NotFound(format!("artifact: {id}")));
        };
        Ok(Artifact {
            id: id.clone(),
            name,
            kind: crate::dec(&kind_s)?,
            checksum_sha256: checksum,
            size_bytes: size as u64,
            created_at,
            meta,
        })
    }

    async fn read(&self, id: &ArtifactId) -> ForgeResult<Vec<u8>> {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT content FROM artifacts WHERE id = $1")
                .bind(id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        row.map(|(c,)| c)
            .ok_or_else(|| ForgeError::NotFound(format!("artifact: {id}")))
    }
}
