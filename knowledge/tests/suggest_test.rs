//! V5.0 KNW-002: 失败知识→回归建议测试

use forge_knowledge::{suggest, InMemoryKnowledgeBase, KnowledgeEntry, FailureKnowledgeBase};
use forge_recovery::classify::classify;
use forge_exec::ExecutionStatus;
use forge_core::{EvidenceId, ExecutionId};

fn make_record(tool: Option<&str>, msg: &str) -> KnowledgeEntry {
    let eid = ExecutionId::new_execution_id();
    let record = classify(&eid, ExecutionStatus::Failed, msg).unwrap();
    KnowledgeEntry {
        record,
        related_evidence: vec![EvidenceId::new_evidence_id()],
        tool: tool.map(|t| t.to_string()),
    }
}

#[tokio::test]
async fn top_n_suggestions_generated() {
    let kb = InMemoryKnowledgeBase::default();
    
    // 插入测试数据
    kb.ingest(make_record(Some("echo"), "tool error 1")).await;
    kb.ingest(make_record(Some("echo"), "tool error 2")).await;
    kb.ingest(make_record(Some("shell"), "timeout")).await;
    
    let suggestions = suggest(&kb, 2).await.unwrap();
    assert_eq!(suggestions.len(), 2, "should return top 2");
}

#[tokio::test]
async fn empty_kb_returns_empty() {
    let kb = InMemoryKnowledgeBase::default();
    let suggestions = suggest(&kb, 5).await.unwrap();
    assert!(suggestions.is_empty(), "empty kb should return no suggestions");
}

#[tokio::test]
async fn write_suggestions_rejects_src_path() {
    let suggestions = vec![forge_knowledge::RegressionSuggestion {
        pattern: "test:echo".into(),
        count: 1,
        suggested_case: serde_json::json!({"tool": "echo"}),
    }];
    
    let result = forge_knowledge::write_suggestions(&suggestions, std::path::Path::new("/tmp/src/test.json")).await;
    assert!(result.is_err(), "should reject writing to src/ directory");
}

#[test]
fn roundtrip_parses() {
    let s = forge_knowledge::RegressionSuggestion {
        pattern: "Test:echo".into(),
        count: 3,
        suggested_case: serde_json::json!({"tool": "echo"}),
    };
    let json = serde_json::to_string(&s).unwrap();
    let decoded: forge_knowledge::RegressionSuggestion = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.pattern, "Test:echo");
    assert_eq!(decoded.count, 3);
}
