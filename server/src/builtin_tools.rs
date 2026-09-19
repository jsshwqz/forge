//! B-REAL-001A：内置工具按白名单接入编排 router（env 门控，缺省零变化）。
//!
//! 把 `parsing/text/zl` 三 crate 的真逻辑工具按 `FORGE_TOOLS_BUILTIN` 环境变量
//! 指定的白名单挂进编排 router。未设置/空白 → 空集 → router 仍恰 5 基线工具。

use std::collections::HashSet;

use forge_exec::{Tool, ToolRouter};

// ── 冻结常量 ──

/// 纯计算壳工具（invoke 返回 pending_engine）：严禁进规划白名单与编排 router。
/// 冻结为常量，不得增删（增删走 R6）。
pub const SHELL_TOOLS: &[&str] = &[
    "pdf_parse",
    "text_embed",
    "text_classify",
    "text_extract",
    "text_summarize",
    "text_translate",
];

/// 内置 5 工具（L1 现状），恒在规划白名单，本包不得移除。
pub const BASE_TOOLS: &[&str] = &["echo", "write_file", "read_file", "list_dir", "edit_patch"];

// ── 公共函数 ──

/// 是否壳工具：命中裸名，或命中桥接全名 `mcp_<server>_<shell>` 后缀
/// （桥接格式冻结见 mcp_tools.rs `bridged_name`）。
pub fn is_shell_tool(name: &str) -> bool {
    // 裸名命中
    if SHELL_TOOLS.contains(&name) {
        return true;
    }
    // 桥接全名：mcp_<server>_<shell>
    if let Some(rest) = name.strip_prefix("mcp_") {
        for shell in SHELL_TOOLS {
            if let Some(server_part) = rest.strip_suffix(shell) {
                // server_part 必须非空且以 '_' 结尾（即 mcp_<server>_<shell> 格式）
                if server_part.ends_with('_') && server_part.len() > 1 {
                    return true;
                }
            }
        }
    }
    false
}

/// env `FORGE_TOOLS_BUILTIN`（逗号分隔）。未设置/空白 → 空集（零变化）。
pub fn builtin_allowlist_from_env() -> HashSet<String> {
    let raw = std::env::var("FORGE_TOOLS_BUILTIN").unwrap_or_default();
    if raw.trim().is_empty() {
        return HashSet::new();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 注册结果摘要。
#[derive(Debug, Clone, Default)]
pub struct BuiltinRegReport {
    /// 实际注册成功（名字）
    pub registered: Vec<String>,
    /// 与已注册重名 → 跳过（不报错，见 R2）
    pub skipped_duplicate: Vec<String>,
    /// 白名单里出现 SHELL_TOOLS → 拒绝
    pub rejected_shell: Vec<String>,
    /// 候选表里没有 → 记 warn
    pub unknown: Vec<String>,
}

/// 按白名单构造并注册（契约冻结签名：不读 env，集合由调用方传入，便于无 env 单测）。
pub fn register_builtin_tools(
    router: &ToolRouter,
    allow: &HashSet<String>,
) -> BuiltinRegReport {
    let mut report = BuiltinRegReport::default();

    for name in allow {
        // R3：壳名优先拒绝（不注册、不报错）
        if is_shell_tool(name) {
            report.rejected_shell.push(name.clone());
            continue;
        }

        // R2：已在 router 注册 → 跳过，不让 InvalidState 冒泡
        if router.route(name).is_ok() {
            report.skipped_duplicate.push(name.clone());
            continue;
        }

        // 候选表查找 + 构造
        match construct_tool(name) {
            Some(tool) => match router.register(tool) {
                Ok(()) => report.registered.push(name.clone()),
                Err(_) => report.skipped_duplicate.push(name.clone()),
            },
            None => report.unknown.push(name.clone()),
        }
    }

    // 排序保证输出确定性（HashSet 迭代序不确定）
    report.registered.sort();
    report.skipped_duplicate.sort();
    report.rejected_shell.sort();
    report.unknown.sort();

    // R4：注册结果摘要单行 eprintln!（无注册时不打日志，防噪音）
    if !report.registered.is_empty() {
        eprintln!(
            "builtin_tools: registered={} skipped={} rejected={} unknown={}",
            report.registered.len(),
            report.skipped_duplicate.len(),
            report.rejected_shell.len(),
            report.unknown.len(),
        );
    }

    report
}

// ── 候选表：20 个真逻辑工具 ──
// 工具名 → 构造函数；逐项照抄不得自造。

fn construct_tool(name: &str) -> Option<Box<dyn Tool>> {
    use forge_tools_parsing::*;
    use forge_tools_text::*;
    use forge_tools_zl::*;

    let tool: Box<dyn Tool> = match name {
        // forge-tools-parsing（4 个真逻辑）
        "csv_parse" => Box::new(CsvParseTool::new()),
        "json_parse" => Box::new(JsonParseTool::new()),
        "yaml_parse" => Box::new(YamlParseTool::new()),
        "toml_parse" => Box::new(TomlParseTool::new()),

        // forge-tools-text（7 个真逻辑；4 个壳不在候选表）
        "markdown_render" => Box::new(MarkdownRenderTool::new()),
        "text_toon" => Box::new(TextToonTool::new()),
        "text_diff" => Box::new(TextDiffTool::new()),
        "text_wordcount" => Box::new(TextWordcountTool::new()),
        "sanitize" => Box::new(SanitizeTool::new()),
        "session_report" => Box::new(SessionReportTool::new()),
        "skill_report" => Box::new(SkillReportTool::new()),

        // forge-tools-zl（9 个真逻辑）
        "check_sufficiency" => Box::new(CheckSufficiencyTool::new()),
        "verify_result" => Box::new(VerifyResultTool::new()),
        "compile_contract" => Box::new(CompileContractTool::new()),
        "detect_drift" => Box::new(DetectDriftTool::new()),
        "contradiction_analyze" => Box::new(ContradictionAnalyzeTool::new()),
        "strategic_plan" => Box::new(StrategicPlanTool::new()),
        "task_dialectic" => Box::new(TaskDialecticTool::new()),
        "dialectical_retry" => Box::new(DialecticalRetryTool::new()),
        "prompt_audit" => Box::new(PromptAuditTool::new()),

        _ => return None,
    };
    Some(tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_tools_constant_frozen() {
        assert_eq!(SHELL_TOOLS.len(), 6);
        assert_eq!(BASE_TOOLS.len(), 5);
    }
}
