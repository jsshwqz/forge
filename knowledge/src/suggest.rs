//! KNW-002: 失败知识→回归建议闭环
//!
//! 从 KnowledgeBase 分析失败模式，生成回归测试建议。

use async_trait::async_trait;
use forge_core::ForgeResult;
use serde::{Deserialize, Serialize};

/// 回归测试建议
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegressionSuggestion {
    pub pattern: String,
    pub count: u64,
    pub suggested_case: serde_json::Value,
}

/// KnowledgeBase trait (简化版)
#[async_trait]
pub trait KnowledgeBase: Send + Sync {
    async fn list_failures(&self) -> ForgeResult<Vec<FailureRecord>>;
}

/// 失败记录
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureRecord {
    pub category: String,
    pub tool: String,
    pub retriable: bool,
    pub summary: String,
}

/// 生成回归建议 (top_n 默认 5)
pub async fn suggest(kb: &dyn KnowledgeBase, top_n: u32) -> ForgeResult<Vec<RegressionSuggestion>> {
    let failures = kb.list_failures().await?;
    
    // 按 pattern 分组
    let mut groups: std::collections::HashMap<String, Vec<&FailureRecord>> = std::collections::HashMap::new();
    for f in failures {
        let pattern = format!("{}:{}", f.category, f.tool);
        groups.entry(pattern).or_default().push(&f);
    }
    
    // 取 top-N
    let mut suggestions: Vec<RegressionSuggestion> = groups.into_iter()
        .map(|(pattern, records)| RegressionSuggestion {
            pattern,
            count: records.len() as u64,
            suggested_case: serde_json::json!({
                "tool": records.first().map(|r| &r.tool).unwrap_or(&String::new()).to_string(),
                "input": "sample_input",
                "expect": "error",
                "from_evidence": vec![]
            }),
        })
        .collect();
    
    suggestions.sort_by(|a, b| b.count.cmp(&a.count));
    suggestions.truncate(top_n as usize);
    
    Ok(suggestions)
}
