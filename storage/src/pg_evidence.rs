//! PgEvidenceStore：EvidenceStore trait 的 PostgreSQL 实现。
//! 证据不可变：仅 INSERT/SELECT，无 UPDATE/DELETE 路径。

use async_trait::async_trait;
use chrono::Utc;
use forge_core::{EvidenceId, ForgeError, ForgeResult};
use forge_evidence::{Evidence, EvidenceKind, EvidenceStore};
use sqlx::PgPool;

/// PostgreSQL 证据存储。
pub struct PgEvidenceStore {
    pool: PgPool,
}

impl PgEvidenceStore {
    /// 用现有连接池构造。
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EvidenceStore for PgEvidenceStore {
    async fn put(&self, mut evidence: Evidence) -> ForgeResult<EvidenceId> {
        // at 零值补齐当前时间（与内存实现同语义）
        if evidence.at == chrono::DateTime::<Utc>::default() {
            evidence.at = Utc::now();
        }
        // 同 ID 重复写入 → InvalidState（先查后插，保持与内存实现一致的错误文案）
        let exists: Option<(String,)> =
            sqlx::query_as("SELECT id FROM evidence WHERE id = $1")
                .bind(evidence.id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        if exists.is_some() {
            return Err(ForgeError::InvalidState(format!(
                "evidence already exists: {}",
                evidence.id
            )));
        }

        sqlx::query(
            "INSERT INTO evidence (id, kind, criterion_id, content, produced_by, at) VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(evidence.id.as_ref())
        .bind(crate::enc(&evidence.kind))
        .bind(&evidence.criterion_id)
        .bind(&evidence.content)
        .bind(&evidence.produced_by)
        .bind(evidence.at)
        .execute(&self.pool)
        .await
        .map_err(crate::db_err)?;
        Ok(evidence.id)
    }

    async fn get(&self, id: &EvidenceId) -> ForgeResult<Evidence> {
        let row: Option<(String, String, String, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT kind, criterion_id, content, produced_by, at FROM evidence WHERE id = $1")
                .bind(id.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(crate::db_err)?;
        let Some((kind_s, criterion_id, content, produced_by, at)) = row else {
            return Err(ForgeError::NotFound(format!("evidence: {id}")));
        };
        Ok(Evidence {
            id: id.clone(),
            kind: crate::dec(&kind_s)?,
            criterion_id,
            content,
            produced_by,
            at,
        })
    }

    async fn by_criterion(&self, criterion_id: &str) -> ForgeResult<Vec<Evidence>> {
        let rows: Vec<(String, String, String, String, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as(
                "SELECT id, kind, criterion_id, content, produced_by, at FROM evidence WHERE criterion_id = $1 ORDER BY at",
            )
            .bind(criterion_id)
            .fetch_all(&self.pool)
            .await
            .map_err(crate::db_err)?;
        rows.into_iter()
            .map(|(id_s, kind_s, criterion_id, content, produced_by, at)| {
                Ok(Evidence {
                    id: EvidenceId::from(id_s),
                    kind: crate::dec::<EvidenceKind>(&kind_s)?,
                    criterion_id,
                    content,
                    produced_by,
                    at,
                })
            })
            .collect()
    }
}
