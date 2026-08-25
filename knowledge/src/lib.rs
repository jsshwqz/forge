//! forge-knowledge：Knowledge 层 MVP（GA KNW-001）。
//!
//! - [`failures`]：失败知识库——FailureRecord+证据聚合，按 category/工具/关键词检索
//! - [`replay`]：Session 回放导出（JSON 归档）
//!
//! 复用既有 store trait（forge-recovery / forge-session），零新存储。

pub mod failures;
pub mod replay;

pub use failures::{
    FailureKnowledgeBase, InMemoryKnowledgeBase, KnowledgeEntry,
};
pub use replay::{archive_to_json, export_replay, ReplayArchive};

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::ExecutionId;
    use forge_exec::ExecutionStatus;
    use forge_recovery::classify::{classify, FailureCategory};
    use forge_session::{InMemorySessionStore, SessionStore as _};

    fn record(msg: &str) -> forge_recovery::classify::FailureRecord {
        let eid = ExecutionId::new_execution_id();
        classify(&eid, ExecutionStatus::Failed, msg).unwrap()
    }

    #[tokio::test]
    async fn ingest_and_filter_by_category_tool_keyword() {
        let kb = InMemoryKnowledgeBase::new();

        let mut r1 = record("tool write_file: disk full");
        r1.category = FailureCategory::ToolError;
        let mut r2 = record("permission denied by policy");
        r2.category = FailureCategory::PermissionDenied;
        r2.retriable = false;

        kb.ingest(KnowledgeEntry { record: r1, related_evidence: vec![], tool: Some("write_file".into()) }).await;
        kb.ingest(KnowledgeEntry { record: r2, related_evidence: vec![], tool: Some("shell".into()) }).await;

        assert_eq!(kb.all().await.len(), 2);

        // 按 category
        let by_cat = kb.search(Some(FailureCategory::PermissionDenied), None, None).await;
        assert_eq!(by_cat.len(), 1);
        // 按工具名
        let by_tool = kb.search(None, Some("write_file"), None).await;
        assert_eq!(by_tool.len(), 1);
        // 关键词（大小写不敏感）
        let by_kw = kb.search(None, None, Some("DISK")).await;
        assert_eq!(by_kw.len(), 1);
        // 组合：category + keyword
        let combo = kb.search(Some(FailureCategory::ToolError), Some("write_file"), Some("disk")).await;
        assert_eq!(combo.len(), 1);
        // 不命中
        assert!(kb.search(Some(FailureCategory::Timeout), None, None).await.is_empty());
    }

    #[tokio::test]
    async fn session_replay_export_roundtrip() {
        let store = InMemorySessionStore::default();
        let sid = store.create(forge_core::TaskId::new_task_id()).await.unwrap().id;
        store.append(&sid, forge_session::SessionEventKind::TaskReceived, serde_json::json!({"hello":true})).await.unwrap();
        store.append(&sid, forge_session::SessionEventKind::PlanCreated, serde_json::json!({"plan_id":"p1"})).await.unwrap();

        let archive = export_replay(&store, &sid).await.unwrap();
        assert_eq!(archive.session.events.len(), 2);
        assert_eq!(archive.format_version, 1);

        let json = archive_to_json(&archive).unwrap();
        assert!(json.contains("PlanCreated"));
        assert!(json.contains("\"hello\":true") || json.contains("hello"));
        // 可反序列化（归档即契约）
        let back: ReplayArchive = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session.id, sid);
    }
}
