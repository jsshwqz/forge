//! 文件持久化知识库（KNW，R7-015 修复）。
//!
//! [`FileKnowledgeBase`]：JSONL append-only 落盘。`ingest` 只追加文件；`all`/`search`
//! 每次从文件读（无内存缓存，跨进程共享失败知识）。让 CLI 独立进程与 server 通过
//! 同一知识文件共享（此前 CLI 读独立空内存库导致 suggest 恒 0 断链）。

use crate::failures::{FailureKnowledgeBase, KnowledgeEntry};
use forge_recovery::classify::FailureCategory;
use std::path::PathBuf;

/// 默认知识库文件：`$FORGE_KNOWLEDGE_FILE` 或 `~/.aion-forge/knowledge.jsonl`。
///
/// 不用系统 temp 目录——temp 会被 OS 清理，"持久化"名不符实。
/// 父目录不存在时自动创建。
pub fn knowledge_file() -> PathBuf {
    if let Ok(f) = std::env::var("FORGE_KNOWLEDGE_FILE") {
        return PathBuf::from(f);
    }
    let dir = dirs_or_home();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("knowledge.jsonl")
}

/// 稳定数据目录：`$HOME/.aion-forge/`，失败时退回 `./.aion-forge/`。
fn dirs_or_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".aion-forge");
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home).join(".aion-forge");
    }
    PathBuf::from(".aion-forge")
}

/// 从 JSONL 文件加载知识条目（容忍坏行，跳过）。
pub fn load_entries(path: &std::path::Path) -> Vec<KnowledgeEntry> {
    let mut out = Vec::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<KnowledgeEntry>(line) {
                Ok(e) => out.push(e),
                Err(_) => eprintln!("[knowledge] skipping malformed JSONL line"),
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
        // JSONL 落盘（同步 std::fs append；失败 warn 不阻断）。
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[knowledge] ingest serialize failed: {e}");
                return;
            }
        };
        // 确保父目录存在
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => {
                use std::io::Write;
                if let Err(e) = writeln!(f, "{line}") {
                    eprintln!("[knowledge] ingest write failed: {e}");
                }
            }
            Err(e) => {
                eprintln!("[knowledge] ingest open failed: {e}");
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

    #[tokio::test]
    async fn file_kb_search_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("search.jsonl");
        let kb = FileKnowledgeBase::new(&path);

        kb.ingest(KnowledgeEntry {
            record: forge_recovery::classify::FailureRecord {
                id: "f-a".into(),
                execution_id: forge_core::ExecutionId::new_execution_id(),
                at: chrono::Utc::now(),
                category: FailureCategory::ToolError,
                message: "disk full".into(),
                retriable: true,
            },
            related_evidence: vec![],
            tool: Some("write_file".into()),
        })
        .await;

        kb.ingest(KnowledgeEntry {
            record: forge_recovery::classify::FailureRecord {
                id: "f-b".into(),
                execution_id: forge_core::ExecutionId::new_execution_id(),
                at: chrono::Utc::now(),
                category: FailureCategory::Timeout,
                message: "slow query".into(),
                retriable: false,
            },
            related_evidence: vec![],
            tool: Some("shell".into()),
        })
        .await;

        assert_eq!(kb.all().await.len(), 2);
        assert_eq!(
            kb.search(Some(FailureCategory::Timeout), None, None)
                .await
                .len(),
            1
        );
        assert_eq!(kb.search(None, Some("write_file"), None).await.len(), 1);
        assert_eq!(kb.search(None, None, Some("disk")).await.len(), 1);
    }

    #[tokio::test]
    async fn file_kb_tolerates_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("malformed.jsonl");
        // 写一行坏数据 + 一行好数据
        std::fs::write(&path, "garbage\n").unwrap();
        let kb = FileKnowledgeBase::new(&path);
        kb.ingest(KnowledgeEntry {
            record: forge_recovery::classify::FailureRecord {
                id: "good".into(),
                execution_id: forge_core::ExecutionId::new_execution_id(),
                at: chrono::Utc::now(),
                category: FailureCategory::ToolError,
                message: "ok".into(),
                retriable: true,
            },
            related_evidence: vec![],
            tool: None,
        })
        .await;
        let all = kb.all().await;
        assert_eq!(all.len(), 1, "should skip malformed, keep good");
    }
}
