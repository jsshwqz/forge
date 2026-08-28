//! V5.0 KNW-002: 失败知识→回归建议测试

use forge_knowledge::suggest::{suggest, KnowledgeBase, FailureRecord, RegressionSuggestion};
use forge_core::ForgeResult;

struct MockKb {
    failures: Vec<FailureRecord>,
}

#[async_trait::async_trait]
impl KnowledgeBase for MockKb {
    async fn list_failures(&self) -> ForgeResult<Vec<FailureRecord>> {
        Ok(self.failures.clone())
    }
}

#[tokio::test]
async fn top_n_suggestions_generated() {
    let kb = MockKb {
        failures: vec![
            FailureRecord { category: "orchestrate".into(), tool: "echo".into(), retriable: false, summary: "fail1".into() },
            FailureRecord { category: "orchestrate".into(), tool: "echo".into(), retriable: true, summary: "fail2".into() },
            FailureRecord { category: "timeout".into(), tool: "shell".into(), retriable: true, summary: "fail3".into() },
        ],
    };
    
    let suggestions = suggest(&kb, 2).await.unwrap();
    assert_eq!(suggestions.len(), 2, "should return top 2");
}

#[test]
fn roundtrip_parses() {
    let s = RegressionSuggestion {
        pattern: "orchestrate:echo".into(),
        count: 2,
        suggested_case: serde_json::json!({"tool": "echo", "input": "test", "expect": "error"}),
    };
    
    let json = serde_json::to_string(&s).unwrap();
    let decoded: RegressionSuggestion = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.pattern, "orchestrate:echo");
    assert_eq!(decoded.count, 2);
}
