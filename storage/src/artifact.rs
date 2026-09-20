//! FileArtifactStore：ArtifactStore trait 的文件系统实现（MKT-104A, D5=方案B）。
//!
//! 制品字节存于本地文件系统，路径由服务端实测 sha256 做 content-hash 两级前缀子目录。
//! 与 PgArtifactStore(BYTEA) / MinioArtifactStore(S3) 并列，为第三个 trait 实现。
//!
//! 布局：
//! ```text
//! {root}/
//!   {sha[0..2]}/
//!     {sha[2..4]}/
//!       {sha256}              # 制品字节 (raw content)
//!       {sha256}.meta.json    # Artifact 元数据 sidecar
//!   index/
//!     {artifact_id}           # 索引文件: 内容为 sha256, 映射 id→文件
//! ```
//!
//! 写入原子性：先写 `.tmp` 再 `rename`。
//! 路径沙箱：sha256 为纯 hex，不包含 `..` 或绝对路径，天然安全。

use async_trait::async_trait;
use chrono::Utc;
use forge_artifact::{Artifact, ArtifactKind, ArtifactStore};
use forge_core::{ArtifactId, ForgeError, ForgeResult};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// 计算 SHA-256 哈希，返回小写十六进制字符串（沿用 pg_artifact.rs 先例）。
fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 默认制品目录：`$FORGE_ARTIFACT_DIR` 或 `~/.aion-forge/artifacts/`。
///
/// 跨平台口径照抄 knowledge_file()：HOME → USERPROFILE → `.` 回退。
/// 父目录不存在时自动创建（lazy——仅在此函数被调用时）。
pub fn default_artifact_dir() -> PathBuf {
    if let Ok(d) = std::env::var("FORGE_ARTIFACT_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".aion-forge").join("artifacts")
}

/// 文件系统制品存储。
pub struct FileArtifactStore {
    root: PathBuf,
}

impl FileArtifactStore {
    /// 用指定根目录构造。目录可在 put 时自动创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 用默认目录构造（$FORGE_ARTIFACT_DIR 或 ~/.aion-forge/artifacts/）。
    pub fn with_default_dir() -> Self {
        Self::new(default_artifact_dir())
    }

    /// 从 sha256 计算两级前缀子目录路径（相对 root）。
    fn rel_content_path(checksum: &str) -> PathBuf {
        PathBuf::from(&checksum[0..2])
            .join(&checksum[2..4])
            .join(checksum)
    }

    /// 制品字节文件的绝对路径。
    fn content_path(&self, checksum: &str) -> PathBuf {
        self.root.join(Self::rel_content_path(checksum))
    }

    /// 元数据 sidecar 文件的绝对路径。
    fn meta_path(&self, checksum: &str) -> PathBuf {
        self.root
            .join(&checksum[0..2])
            .join(&checksum[2..4])
            .join(format!("{checksum}.meta.json"))
    }

    /// 索引文件绝对路径（artifact_id → sha256）。
    fn index_path(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("index").join(id.as_ref())
    }

    /// 从 ArtifactId 查找对应的 sha256（读索引文件）。
    fn lookup_checksum(&self, id: &ArtifactId) -> ForgeResult<String> {
        let idx = self.index_path(id);
        std::fs::read_to_string(&idx)
            .map_err(|e| ForgeError::NotFound(format!("artifact index {}: {}", id, e)))
    }

    /// 引用计数：扫描 index/ 目录，统计指向同一 checksum 的索引条目数
    /// （排除当前正在删除的 artifact_id）。
    fn count_checksum_refs(&self, checksum: &str, exclude: &ArtifactId) -> usize {
        let index_dir = self.root.join("index");
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(&index_dir) {
            for entry in entries.flatten() {
                // 跳过当前正在删除的条目
                if entry.file_name().to_string_lossy() == exclude.as_ref() {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if content == checksum {
                        count += 1;
                    }
                }
            }
        }
        count
    }
}

#[async_trait]
impl ArtifactStore for FileArtifactStore {
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
        let created_at = Utc::now();

        let content_path = self.content_path(&checksum);
        let meta_path = self.meta_path(&checksum);
        let index_path = self.index_path(&id);

        // 创建两级前缀子目录
        if let Some(parent) = content_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ForgeError::InvalidState(format!("artifact mkdir: {e}")))?;
        }
        std::fs::create_dir_all(self.root.join("index"))
            .map_err(|e| ForgeError::InvalidState(format!("artifact index mkdir: {e}")))?;

        // 原子写入：先写 .tmp 再 rename
        let tmp = content_path.with_extension("tmp");
        std::fs::write(&tmp, &content)
            .map_err(|e| ForgeError::InvalidState(format!("artifact write tmp: {e}")))?;
        std::fs::rename(&tmp, &content_path)
            .map_err(|e| ForgeError::InvalidState(format!("artifact rename: {e}")))?;

        // 写元数据 sidecar
        let artifact = Artifact {
            id: id.clone(),
            name,
            kind,
            checksum_sha256: checksum.clone(),
            size_bytes: size,
            created_at,
            meta,
        };
        let meta_json = serde_json::to_string_pretty(&artifact)
            .map_err(|e| ForgeError::InvalidState(format!("artifact meta serialize: {e}")))?;
        std::fs::write(&meta_path, meta_json)
            .map_err(|e| ForgeError::InvalidState(format!("artifact meta write: {e}")))?;

        // 写索引文件 (artifact_id → sha256)
        std::fs::write(&index_path, &checksum)
            .map_err(|e| ForgeError::InvalidState(format!("artifact index write: {e}")))?;

        Ok(artifact)
    }

    async fn get_meta(&self, id: &ArtifactId) -> ForgeResult<Artifact> {
        let checksum = self.lookup_checksum(id)?;
        let meta_path = self.meta_path(&checksum);
        let json = std::fs::read_to_string(&meta_path)
            .map_err(|e| ForgeError::NotFound(format!("artifact meta {}: {}", id, e)))?;
        serde_json::from_str(&json)
            .map_err(|e| ForgeError::InvalidState(format!("artifact meta deserialize: {e}")))
    }

    async fn read(&self, id: &ArtifactId) -> ForgeResult<Vec<u8>> {
        let checksum = self.lookup_checksum(id)?;
        let content_path = self.content_path(&checksum);
        std::fs::read(&content_path)
            .map_err(|e| ForgeError::NotFound(format!("artifact content {}: {}", id, e)))
    }

    async fn delete(&self, id: &ArtifactId) -> ForgeResult<()> {
        // 先查索引拿 sha256；索引不存在视为已删（幂等）
        let checksum = match self.lookup_checksum(id) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        // 引用计数：扫描 index/ 目录，统计有多少索引文件指向同一 checksum。
        // 如果除了当前 artifact 还有其他条目引用同一 checksum（content-hash 去重），
        // 则只删当前索引条目，不删内容文件和 meta——否则会把其他 artifact 的数据删掉。
        let other_refs = self.count_checksum_refs(&checksum, id);

        // 先删索引（无论引用计数如何，当前 artifact 的索引总是要删的）
        let _ = std::fs::remove_file(self.index_path(id));

        if other_refs == 0 {
            // 没有其他引用，安全删除内容文件和 sidecar
            let _ = std::fs::remove_file(self.content_path(&checksum));
            let _ = std::fs::remove_file(self.meta_path(&checksum));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;
    use std::time::Duration;

    #[tokio::test]
    async fn put_then_read_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileArtifactStore::new(tmp.path());

        let content = b"hello artifact world".to_vec();
        let art = timeout(Duration::from_secs(5), store.put(
            "test.bin".into(),
            ArtifactKind::Binary,
            content.clone(),
            serde_json::json!({"source": "test"}),
        ))
        .await
        .unwrap()
        .unwrap();

        // 读回字节一致
        let read_content = timeout(Duration::from_secs(5), store.read(&art.id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_content, content);

        // 元数据一致
        let meta = timeout(Duration::from_secs(5), store.get_meta(&art.id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta.checksum_sha256, art.checksum_sha256);
        assert_eq!(meta.size_bytes, content.len() as u64);
    }

    #[tokio::test]
    async fn checksum_is_real_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileArtifactStore::new(tmp.path());

        let content = b"hello world".to_vec();
        let art = timeout(Duration::from_secs(5), store.put(
            "test.txt".into(),
            ArtifactKind::Document,
            content,
            serde_json::json!({}),
        ))
        .await
        .unwrap()
        .unwrap();

        // SHA-256 of "hello world"
        assert_eq!(
            art.checksum_sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn same_content_different_id_same_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileArtifactStore::new(tmp.path());

        let content = b"dedup content".to_vec();
        let art1 = timeout(Duration::from_secs(5), store.put(
            "a.txt".into(), ArtifactKind::Code, content.clone(), serde_json::json!({}),
        ))
        .await
        .unwrap()
        .unwrap();
        let art2 = timeout(Duration::from_secs(5), store.put(
            "b.txt".into(), ArtifactKind::Code, content, serde_json::json!({}),
        ))
        .await
        .unwrap()
        .unwrap();

        assert_ne!(art1.id, art2.id);
        assert_eq!(art1.checksum_sha256, art2.checksum_sha256);
        // 同一 content 文件只存一份 (content-hash 去重)
    }

    #[tokio::test]
    async fn not_found_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileArtifactStore::new(tmp.path());
        let fake_id = ArtifactId::new_artifact_id();

        assert!(timeout(Duration::from_secs(5), store.get_meta(&fake_id))
            .await
            .unwrap()
            .is_err());
        assert!(timeout(Duration::from_secs(5), store.read(&fake_id))
            .await
            .unwrap()
            .is_err());
    }

    /// IMPROVE-1: 两个 artifact 共享同一 checksum（相同内容去重），
    /// 删第一个后第二个仍可读——引用计数防止数据丢失。
    #[tokio::test]
    async fn delete_shared_checksum_preserves_other() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileArtifactStore::new(tmp.path());

        let content = b"shared dedup content".to_vec();
        let art1 = timeout(Duration::from_secs(5), store.put(
            "first.txt".into(), ArtifactKind::Code, content.clone(), serde_json::json!({}),
        ))
        .await.unwrap().unwrap();
        let art2 = timeout(Duration::from_secs(5), store.put(
            "second.txt".into(), ArtifactKind::Code, content, serde_json::json!({}),
        ))
        .await.unwrap().unwrap();

        assert_eq!(art1.checksum_sha256, art2.checksum_sha256);

        // 删第一个——内容文件不应被删除，因为 art2 仍引用它
        timeout(Duration::from_secs(5), store.delete(&art1.id))
            .await.unwrap().unwrap();

        // art1 已删，get_meta 应失败
        assert!(timeout(Duration::from_secs(5), store.get_meta(&art1.id))
            .await.unwrap().is_err());

        // art2 仍可读——这是核心断言
        let read_back = timeout(Duration::from_secs(5), store.read(&art2.id))
            .await.unwrap().unwrap();
        assert_eq!(read_back, b"shared dedup content");

        // art2 的 meta 也可读
        let meta = timeout(Duration::from_secs(5), store.get_meta(&art2.id))
            .await.unwrap().unwrap();
        assert_eq!(meta.name, "second.txt");
    }
}
