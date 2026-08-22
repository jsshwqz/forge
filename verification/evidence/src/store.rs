//! 证据存储接口与内存实现。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use forge_core::{EvidenceId, ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 证据类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    /// 命令输出。
    CommandOutput,
    /// 文件内容。
    FileContent,
    /// 日志。
    Log,
    /// 测试报告。
    TestReport,
}

/// 证据对象。不可变：存储不提供任何更新/删除接口。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    /// 证据 ID。
    pub id: EvidenceId,
    /// 证据类型。
    pub kind: EvidenceKind,
    /// 关联的验收标准 ID。
    pub criterion_id: String,
    /// 内容（第一阶段存文本；大文件第二阶段走 Artifact）。
    pub content: String,
    /// 产生者（验证器名，如 "CommandVerifier"）。
    pub produced_by: String,
    /// 产生时间。
    pub at: DateTime<Utc>,
}

/// 证据存储 trait。
#[async_trait]
pub trait EvidenceStore: Send + Sync {
    /// 写入证据。同 ID 重复写入 → `InvalidState`。
    /// `at` 若为零值由存储补齐为当前时间；非零则原样保留。
    async fn put(&self, evidence: Evidence) -> ForgeResult<EvidenceId>;

    /// 获取证据。
    async fn get(&self, id: &EvidenceId) -> ForgeResult<Evidence>;

    /// 按验收标准 ID 查询证据。
    async fn by_criterion(&self, criterion_id: &str) -> ForgeResult<Vec<Evidence>>;
}

/// 内存证据存储。
#[derive(Default)]
pub struct InMemoryEvidenceStore {
    evidence: Arc<RwLock<HashMap<EvidenceId, Evidence>>>,
}

#[async_trait]
impl EvidenceStore for InMemoryEvidenceStore {
    async fn put(&self, mut evidence: Evidence) -> ForgeResult<EvidenceId> {
        let id = evidence.id.clone();

        // at 为零值时补齐当前时间
        if evidence.at == DateTime::<Utc>::default() {
            evidence.at = Utc::now();
        }

        let mut guard = self.evidence.write().await;
        if guard.contains_key(&id) {
            return Err(ForgeError::InvalidState(format!(
                "evidence already exists: {}",
                id
            )));
        }
        guard.insert(id.clone(), evidence);
        Ok(id)
    }

    async fn get(&self, id: &EvidenceId) -> ForgeResult<Evidence> {
        self.evidence
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("evidence: {}", id)))
    }

    async fn by_criterion(&self, criterion_id: &str) -> ForgeResult<Vec<Evidence>> {
        Ok(self
            .evidence
            .read()
            .await
            .values()
            .filter(|e| e.criterion_id == criterion_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_evidence(criterion_id: &str) -> Evidence {
        Evidence {
            id: EvidenceId::new_evidence_id(),
            kind: EvidenceKind::CommandOutput,
            criterion_id: criterion_id.into(),
            content: "test output".into(),
            produced_by: "CommandVerifier".into(),
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let store = InMemoryEvidenceStore::default();
        let ev = make_evidence("AC-1");
        let id = store.put(ev.clone()).await.unwrap();
        let got = store.get(&id).await.unwrap();
        assert_eq!(got.criterion_id, "AC-1");
        assert_eq!(got.content, "test output");
    }

    #[tokio::test]
    async fn test_duplicate_id_rejected() {
        let store = InMemoryEvidenceStore::default();
        let ev = make_evidence("AC-1");
        let id = store.put(ev.clone()).await.unwrap();

        // 用相同 ID 再次写入
        let mut ev2 = make_evidence("AC-2");
        ev2.id = id;
        let result = store.put(ev2).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_by_criterion() {
        let store = InMemoryEvidenceStore::default();
        store.put(make_evidence("AC-1")).await.unwrap();
        store.put(make_evidence("AC-1")).await.unwrap();
        store.put(make_evidence("AC-2")).await.unwrap();

        let results = store.by_criterion("AC-1").await.unwrap();
        assert_eq!(results.len(), 2);

        let results = store.by_criterion("AC-2").await.unwrap();
        assert_eq!(results.len(), 1);

        let results = store.by_criterion("AC-3").await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_zero_at_filled() {
        let store = InMemoryEvidenceStore::default();
        let mut ev = make_evidence("AC-1");
        ev.at = DateTime::<Utc>::default();
        let id = store.put(ev).await.unwrap();
        let got = store.get(&id).await.unwrap();
        assert_ne!(got.at, DateTime::<Utc>::default());
    }

    #[tokio::test]
    async fn test_nonzero_at_preserved() {
        let store = InMemoryEvidenceStore::default();
        let fixed_time = chrono::TimeZone::with_ymd_and_hms(&Utc, 2025, 1, 1, 0, 0, 0).unwrap();
        let mut ev = make_evidence("AC-1");
        ev.at = fixed_time;
        let id = store.put(ev).await.unwrap();
        let got = store.get(&id).await.unwrap();
        assert_eq!(got.at, fixed_time);
    }

    #[tokio::test]
    async fn test_not_found() {
        let store = InMemoryEvidenceStore::default();
        assert!(store.get(&EvidenceId::new_evidence_id()).await.is_err());
    }
}
