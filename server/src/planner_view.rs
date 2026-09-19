//! B-REAL-001B 决策面：规划白名单由 router 派生 + input schema 注入。
//!
//! 闭合 L2/L3——决策面看到的工具集合 = 装配面实际注册的集合（单一真相），
//! 并让模型知道每个工具的 input 形状。

use forge_exec::ToolRouter;
use serde_json;

use crate::builtin_tools::{is_shell_tool, BASE_TOOLS};

/// 工具 input schema 注入块总量上限（防提示词爆炸，对齐 CONTEXT_MAX_BYTES 同类风险）。
pub const SCHEMA_HINT_MAX_BYTES: usize = 8 * 1024;

/// 规划可见工具集 = BASE_TOOLS(恒在) ∪ (router 实际注册名 \ SHELL_TOOLS)，按名升序。
pub fn planner_tool_names(router: &ToolRouter) -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = BASE_TOOLS.iter().map(|s| s.to_string()).collect();

    for desc in router.list() {
        if !is_shell_tool(&desc.name) {
            names.insert(desc.name.clone());
        }
    }

    names.into_iter().collect()
}

/// 生成注入块；首行冻结为 "=== Tool input schemas ==="，其后每行 "<name>: <input_schema 紧凑JSON>"。
/// 超 SCHEMA_HINT_MAX_BYTES → 截断并以 "(truncated)" 收尾。names 为空 → 返回空串。
pub fn build_tool_schema_hint(router: &ToolRouter, names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }

    let header = "=== Tool input schemas ===\n";
    let mut buf = String::with_capacity(SCHEMA_HINT_MAX_BYTES + 64);
    buf.push_str(header);

    for name in names {
        let line = match router.route(name) {
            Ok(tool) => {
                let schema = serde_json::to_string(&tool.descriptor().input_schema)
                    .unwrap_or_else(|_| "{}".to_string());
                format!("{name}: {schema}\n")
            }
            Err(_) => continue, // 工具不在 router 中（理论不发生，跳过）
        };

        // 预算检查：加这行会不会超？
        if buf.len() + line.len() > SCHEMA_HINT_MAX_BYTES {
            buf.push_str("(truncated)");
            break;
        }
        buf.push_str(&line);
    }

    // 如果所有行都加完了还没超预算，且没有截断标记，正常返回
    buf
}
