//! Artifact 存储接口与内存实现。

use async_trait::async_trait;
use chrono::Utc;
use forge_core::{ArtifactId, ForgeResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 产物类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    Code,
    Document,
    Log,
    TestReport,
    Binary,
    Other,
}

/// 产物对象。不可变：put 之后没有任何修改入口。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    /// 产物 ID。
    pub id: ArtifactId,
    /// 名称。
    pub name: String,
    /// 类型。
    pub kind: ArtifactKind,
    /// 内容 SHA-256 哈希，小写十六进制。
    pub checksum_sha256: String,
    /// 内容大小（字节）。
    pub size_bytes: u64,
    /// 创建时间。
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 元数据。
    pub meta: serde_json::Value,
}

/// 产物存储 trait。
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// 存入产物，内部计算 checksum。调用方不传入哈希。
    async fn put(
        &self,
        name: String,
        kind: ArtifactKind,
        content: Vec<u8>,
        meta: serde_json::Value,
    ) -> ForgeResult<Artifact>;

    /// 获取产物元数据。
    async fn get_meta(&self, id: &ArtifactId) -> ForgeResult<Artifact>;

    /// 读取产物内容。
    async fn read(&self, id: &ArtifactId) -> ForgeResult<Vec<u8>>;
}

/// 内存产物存储中的条目：元数据 + 内容。
type ArtifactEntry = (Artifact, Vec<u8>);

/// 内存产物存储。
#[derive(Default)]
pub struct InMemoryArtifactStore {
    artifacts: Arc<RwLock<HashMap<ArtifactId, ArtifactEntry>>>,
}

/// 计算 SHA-256 哈希，返回小写十六进制字符串。
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(&hasher.finalize())
}

// 简易 hex 编码（避免引入 hex crate）
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[async_trait]
impl ArtifactStore for InMemoryArtifactStore {
    async fn put(
        &self,
        name: String,
        kind: ArtifactKind,
        content: Vec<u8>,
        meta: serde_json::Value,
    ) -> ForgeResult<Artifact> {
        let id = ArtifactId::new_artifact_id();
        let checksum = sha256_hex(&content);
        let size = content.len() as u64;
        let artifact = Artifact {
            id: id.clone(),
            name,
            kind,
            checksum_sha256: checksum,
            size_bytes: size,
            created_at: Utc::now(),
            meta,
        };
        self.artifacts
            .write()
            .await
            .insert(id.clone(), (artifact.clone(), content));
        Ok(artifact)
    }

    async fn get_meta(&self, id: &ArtifactId) -> ForgeResult<Artifact> {
        self.artifacts
            .read()
            .await
            .get(id)
            .map(|(a, _)| a.clone())
            .ok_or_else(|| forge_core::ForgeError::NotFound(format!("artifact: {}", id)))
    }

    async fn read(&self, id: &ArtifactId) -> ForgeResult<Vec<u8>> {
        self.artifacts
            .read()
            .await
            .get(id)
            .map(|(_, data)| data.clone())
            .ok_or_else(|| forge_core::ForgeError::NotFound(format!("artifact: {}", id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_put_and_read() {
        let store = InMemoryArtifactStore::default();
        let content = b"hello world".to_vec();
        let art = store
            .put("test.txt".into(), ArtifactKind::Document, content.clone(), serde_json::json!({}))
            .await
            .unwrap();

        let read_content = store.read(&art.id).await.unwrap();
        assert_eq!(read_content, content);
    }

    #[tokio::test]
    async fn test_checksum_correctness() {
        let store = InMemoryArtifactStore::default();
        let content = b"hello world".to_vec();
        let art = store
            .put("test.txt".into(), ArtifactKind::Document, content, serde_json::json!({}))
            .await
            .unwrap();

        // SHA-256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(art.checksum_sha256, expected);
    }

    #[tokio::test]
    async fn test_same_content_different_id_same_checksum() {
        let store = InMemoryArtifactStore::default();
        let content = b"same content".to_vec();

        let art1 = store
            .put("a.txt".into(), ArtifactKind::Code, content.clone(), serde_json::json!({}))
            .await
            .unwrap();
        let art2 = store
            .put("b.txt".into(), ArtifactKind::Code, content, serde_json::json!({}))
            .await
            .unwrap();

        assert_ne!(art1.id, art2.id);
        assert_eq!(art1.checksum_sha256, art2.checksum_sha256);
    }

    #[tokio::test]
    async fn test_size_bytes() {
        let store = InMemoryArtifactStore::default();
        let content = vec![0u8; 1024];
        let art = store
            .put("data.bin".into(), ArtifactKind::Binary, content, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(art.size_bytes, 1024);
    }

    #[tokio::test]
    async fn test_not_found() {
        let store = InMemoryArtifactStore::default();
        let fake_id = ArtifactId::new_artifact_id();
        assert!(store.get_meta(&fake_id).await.is_err());
        assert!(store.read(&fake_id).await.is_err());
    }

    #[tokio::test]
    async fn test_meta_preserved() {
        let store = InMemoryArtifactStore::default();
        let meta = serde_json::json!({"author": "agent", "version": "1.0"});
        let art = store
            .put("doc.md".into(), ArtifactKind::Document, b"content".to_vec(), meta.clone())
            .await
            .unwrap();
        assert_eq!(art.meta, meta);

        let got = store.get_meta(&art.id).await.unwrap();
        assert_eq!(got.meta, meta);
    }
}
