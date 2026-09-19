//! 文件持久化知识库（KNW，R7-015 修复）。
//!
//! [`FileKnowledgeBase`]：JSONL append-only 落盘。`ingest` 只追加文件；`all`/`search`
//! 每次从文件读（无内存缓存，跨进程共享失败知识）。让 CLI 独立进程与 server 通过
//! 同一知识文件共享（此前 CLI 读独立空内存库导致 suggest 恒 0 断链）。

use crate::failures::{FailureKnowledgeBase, KnowledgeEntry};
use forge_recovery::classify::FailureCategory;
use std::path::PathBuf;

/// 默认知识库文件：`$FORGE_KNOWLEDGE_FILE` 或系统临时目录 `forge-knowledge.jsonl`。
pub fn knowledge_file() -> PathBuf {
    std::env::var("FORGE_KNOWLEDGE_FILE")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("forge-knowledge.jsonl"))
}

/// 从 JSONL 文件加载知识条目（容忍坏行，跳过）。
pub fn load_entries(path: &std::path::Path) -> Vec<KnowledgeEntry> {
    let mut out = Vec::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_str::<KnowledgeEntry>(line) {
                out.push(e);
            }
        }
    }
    out
}

/// 文件持久化知识库。
pub struct FileKnowledgeBase {
    path: PathBuf,
}

impl FileKnowledgeBase {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    async fn load(&self) -> Vec<KnowledgeEntry> {
        load_entries(&self.path)
    }
}

#[async_trait::async_trait]
impl FailureKnowledgeBase for FileKnowledgeBase {
    async fn ingest(&self, entry: KnowledgeEntry) {
        // JSONL 落盘（同步 std::fs append；失败仅忽略不阻断）。
        if let Ok(line) = serde_json::to_string(&entry) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                use std::io::Write;
                let _ = writeln!(f, "{line}");
            }
        }
    }

    async fn search(
        &self,
        category: Option<FailureCategory>,
        tool: Option<&str>,
        keyword: Option<&str>,
    ) -> Vec<KnowledgeEntry> {
        self.load()
            .await
            .into_iter()
            .filter(|e| e.matches(category, tool, keyword))
            .collect()
    }

    async fn all(&self) -> Vec<KnowledgeEntry> {
        self.load().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_kb_persists_across_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("kb.jsonl");

        // 实例 A 写入
        let a = FileKnowledgeBase::new(&path);
        a.ingest(KnowledgeEntry {
            record: forge_recovery::classify::FailureRecord {
                id: "fail-1".into(),
                execution_id: forge_core::ExecutionId::new_execution_id(),
                at: chrono::Utc::now(),
                category: FailureCategory::Timeout,
                message: "timeout".into(),
                retriable: true,
            },
            related_evidence: vec![],
            tool: Some("echo".into()),
        })
        .await;

        // 实例 B（模拟独立进程）读
        let b = FileKnowledgeBase::new(&path);
        let all = b.all().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tool.as_deref(), Some("echo"));
    }
}
