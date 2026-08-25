//! 失败知识库（KNW-001）：FailureRecord + 关联证据 → 可检索条目。
//!
//! 检索维度（契约 10.2）：按 `category`、按工具名、按消息关键词。
//! 复用 forge-recovery 的 FailureRecord，不引入新存储格式。

use forge_recovery::classify::{FailureCategory, FailureRecord};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 一条知识条目：失败记录 + 关联证据 + 归因工具。
#[derive(Clone, Debug, Serialize)]
pub struct KnowledgeEntry {
    pub record: FailureRecord,
    /// 关联证据 ID（如失败步骤产生的日志/输出）。
    pub related_evidence: Vec<forge_core::EvidenceId>,
    /// 归因工具名（调用方从执行上下文提取；未知为 None）。
    pub tool: Option<String>,
}

impl KnowledgeEntry {
    /// 命中过滤条件？（全部 Some 的条件都必须满足）
    fn matches(
        &self,
        category: Option<FailureCategory>,
        tool: Option<&str>,
        keyword: Option<&str>,
    ) -> bool {
        if let Some(c) = category {
            if self.record.category != c {
                return false;
            }
        }
        if let Some(t) = tool {
            if self.tool.as_deref() != Some(t) {
                return false;
            }
        }
        if let Some(k) = keyword {
            if !self.record.message.to_lowercase().contains(&k.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// 知识库 trait（便于后续接 PG 实现）。
#[async_trait::async_trait]
pub trait FailureKnowledgeBase: Send + Sync {
    async fn ingest(&self, entry: KnowledgeEntry);
    async fn search(
        &self,
        category: Option<FailureCategory>,
        tool: Option<&str>,
        keyword: Option<&str>,
    ) -> Vec<KnowledgeEntry>;
    async fn all(&self) -> Vec<KnowledgeEntry>;
}

/// 内存实现。
#[derive(Default)]
pub struct InMemoryKnowledgeBase {
    entries: Arc<RwLock<Vec<KnowledgeEntry>>>,
    /// 工具名索引：tool -> entry ids（简化 MVP；量大再换正式索引）。
    by_tool: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl InMemoryKnowledgeBase {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl FailureKnowledgeBase for InMemoryKnowledgeBase {
    async fn ingest(&self, entry: KnowledgeEntry) {
        if let Some(t) = &entry.tool {
            self.by_tool
                .write()
                .await
                .entry(t.clone())
                .or_default()
                .push(entry.record.id.clone());
        }
        self.entries.write().await.push(entry);
    }

    async fn search(
        &self,
        category: Option<FailureCategory>,
        tool: Option<&str>,
        keyword: Option<&str>,
    ) -> Vec<KnowledgeEntry> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|e| e.matches(category, tool, keyword))
            .cloned()
            .collect()
    }

    async fn all(&self) -> Vec<KnowledgeEntry> {
        self.entries.read().await.clone()
    }
}
