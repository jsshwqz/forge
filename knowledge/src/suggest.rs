//! V5.0 KNW-002: 失败知识→回归建议闭环
//!
//! 从失败知识库生成回归用例建议。
//! **安全红线**: 本功能永不修改源码/测试文件，仅输出建议到指定文件。

use crate::failures::{FailureKnowledgeBase, KnowledgeEntry};
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// 回归建议结构
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegressionSuggestion {
    /// 模式标识: "<category>:<tool>"
    pub pattern: String,
    /// 该模式出现次数
    pub count: u64,
    /// 建议的回归用例 JSON
    pub suggested_case: serde_json::Value,
}

/// 从 KnowledgeBase 取 top-N 组，每组生成一条回归建议
/// N 默认 5
pub async fn suggest(
    kb: &dyn FailureKnowledgeBase,
    top_n: u32,
) -> ForgeResult<Vec<RegressionSuggestion>> {
    // 使用 all() 方法获取所有条目
    let entries = kb.all().await;
    
    // 按 category:tool 分组统计
    let mut groups: HashMap<(String, String), Vec<&KnowledgeEntry>> = HashMap::new();
    for entry in &entries {
        let key = (entry.record.category.clone(), entry.tool.clone());
        groups.entry(key).or_default().push(entry);
    }
    
    // 取 top-N 组
    let mut sorted: Vec<_> = groups.into_values().collect();
    sorted.sort_by_key(|v| std::cmp::Reverse(v.len()));
    sorted.truncate(top_n as usize);
    
    let mut suggestions = Vec::new();
    for group in sorted {
        if group.is_empty() { continue; }
        
        let first = group[0];
        let pattern = format!("{}:{}", first.record.category, first.tool);
        
        // 生成建议用例
        let suggested_case = serde_json::json!({
            "tool": first.tool,
            "category": first.record.category,
            "input": "sample_input",
            "expect": if first.record.retriable { "timeout" } else { "error" },
            "count": group.len(),
            "from_evidence": first.related_evidence.iter()
                .map(|e| e.id.to_string())
                .collect::<Vec<_>>()
        });
        
        suggestions.push(RegressionSuggestion {
            pattern,
            count: group.len() as u64,
            suggested_case,
        });
    }
    
    Ok(suggestions)
}

/// 将建议写入文件（不触碰 src/ 或 tests/）
pub async fn write_suggestions(
    suggestions: &[RegressionSuggestion],
    out_path: &Path,
) -> ForgeResult<()> {
    // 安全检查：确保不写入 src/ 或 tests/
    let path_str = out_path.to_string_lossy();
    if path_str.contains("/src/") || path_str.contains("/tests/") {
        return Err(ForgeError::InvalidState(
            "safety: cannot write suggestions to src/ or tests/ directory".into()
        ));
    }
    
    let json = serde_json::to_string_pretty(suggestions)
        .map_err(|e| ForgeError::InvalidState(e.to_string()))?;
    std::fs::write(out_path, json)
        .map_err(|e| ForgeError::InvalidState(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryKnowledgeBase;
    
    #[tokio::test]
    async fn top_n_suggestions_generated() {
        let kb = InMemoryKnowledgeBase::default();
        let result = suggest(&kb, 5).await.unwrap();
        assert!(result.len() <= 5);
    }
    
    #[test]
    fn roundtrip_parses() {
        let s = RegressionSuggestion {
            pattern: "test:echo".into(),
            count: 3,
            suggested_case: serde_json::json!({"tool": "echo"}),
        };
        let json = serde_json::to_string(&s).unwrap();
        let decoded: RegressionSuggestion = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.pattern, "test:echo");
    }
}
