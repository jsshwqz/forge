//! Forge MCP Server：把 Forge 内置工具通过 MCP stdio 协议对外暴露。
//!
//! 协议版本：2024-11-05（与 forge-mcp 客户端一致）。
//! 传输：行分隔 JSON-RPC 2.0 over stdio。
//!
//! 支持的方法：
//! - initialize → 返回 serverInfo + capabilities
//! - notifications/initialized → 确认握手（通知，无响应）
//! - tools/list → 返回 ToolRouter 中所有已注册工具
//! - tools/call → 路由到对应 Tool::invoke
//!
//! 工具注册：启动时注册 BASE_TOOLS(5) + 内置白名单（由 FORGE_TOOLS_BUILTIN 控制）。
//! 工具构造逻辑与 server/src/builtin_tools.rs 的 construct_tool 保持一致。
//!
//! 用法：
//!   forge-mcp-server
//!   FORGE_TOOLS_BUILTIN=csv_parse,markdown_render forge-mcp-server
//!
//! 被 Forge MCP 客户端消费的配置示例：
//!   FORGE_MCP_SERVERS='[{"name":"forge","command":"forge-mcp-server","args":[],"env":{}}]'
//!   FORGE_MCP_ALLOWLIST=forge_echo,forge_csv_parse

use std::io::{BufRead, Write};

use forge_exec::{Tool, ToolDescriptor, ToolRouter};
use forge_mcp::jsonrpc::PROTOCOL_VERSION;

// ── 工具构造（与 server/src/builtin_tools.rs::construct_tool 保持一致） ──

/// 内置 5 工具（基线），恒注册。
const BASE_TOOL_NAMES: &[&str] = &["echo", "write_file", "read_file", "list_dir", "edit_patch"];

/// 壳工具名（不注册，与 server 的 SHELL_TOOLS 一致）。
const SHELL_TOOLS: &[&str] = &[
    "pdf_parse",
    "text_embed",
    "text_classify",
    "text_extract",
    "text_summarize",
    "text_translate",
];

fn is_shell_tool(name: &str) -> bool {
    SHELL_TOOLS.contains(&name)
}

/// 构造工具实例（与 server builtin_tools::construct_tool 同源）。
fn construct_tool(name: &str, workspace_root: &std::path::Path) -> Option<Box<dyn Tool>> {
    use forge_tools_parsing::*;
    use forge_tools_text::*;
    use forge_tools_zl::*;

    let tool: Box<dyn Tool> = match name {
        // BASE_TOOLS（在 execution/runtime 中定义）
        "echo" => Box::new(forge_exec::EchoTool::new()),
        "write_file" => Box::new(forge_exec::WriteFileTool::new(workspace_root.to_path_buf())),
        "read_file" => Box::new(forge_exec::ReadFileTool::new(workspace_root.to_path_buf())),
        "list_dir" => Box::new(forge_exec::ListDirTool::new(workspace_root.to_path_buf())),
        "edit_patch" => Box::new(forge_exec::EditPatchTool::new(workspace_root.to_path_buf())),

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

// ── JSON-RPC 响应构造 ──

fn respond(id: &serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn respond_error(id: &serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

// ── ToolRouter 构建 ──

/// 构建 ToolRouter 并注册所有可用工具。
///
/// 注册策略：
/// 1. BASE_TOOL_NAMES(5) 恒注册（echo, write_file, read_file, list_dir, edit_patch）
/// 2. FORGE_TOOLS_BUILTIN 白名单工具按 env 注册（csv_parse, markdown_render 等）
/// 3. 壳工具（SHELL_TOOLS）一律跳过
fn build_router() -> ToolRouter {
    // 工作区根目录：FORGE_WORKSPACE 环境变量优先，缺省 "."（当前目录）。
    let workspace_root = std::path::PathBuf::from(
        std::env::var("FORGE_WORKSPACE").unwrap_or_else(|_| ".".into())
    );
    let router = ToolRouter::new();
    let mut registered = Vec::new();
    let mut skipped = Vec::new();
    let mut rejected = Vec::new();
    let mut unknown = Vec::new();

    // 注册 BASE_TOOLS
    for name in BASE_TOOL_NAMES {
        if router.route(name).is_ok() {
            skipped.push(*name);
            continue;
        }
        match construct_tool(name, &workspace_root) {
            Some(tool) => match router.register(tool) {
                Ok(()) => registered.push(*name),
                Err(_) => skipped.push(*name),
            },
            None => unknown.push(*name),
        }
    }

    // 注册 env 白名单工具
    let builtin_raw = std::env::var("FORGE_TOOLS_BUILTIN").unwrap_or_default();
    if !builtin_raw.trim().is_empty() {
        for name in builtin_raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if is_shell_tool(name) {
                rejected.push(name);
                continue;
            }
            if router.route(name).is_ok() {
                skipped.push(name);
                continue;
            }
            match construct_tool(name, &workspace_root) {
                Some(tool) => match router.register(tool) {
                    Ok(()) => registered.push(name),
                    Err(_) => skipped.push(name),
                },
                None => unknown.push(name),
            }
        }
    }

    eprintln!(
        "forge-mcp-server: registered={} skipped={} rejected={} unknown={}",
        registered.len(),
        skipped.len(),
        rejected.len(),
        unknown.len(),
    );
    if !registered.is_empty() {
        eprintln!("  registered: {}", registered.join(", "));
    }
    if !unknown.is_empty() {
        eprintln!("  unknown: {}", unknown.join(", "));
    }

    router
}

/// 将 ToolDescriptor 转为 MCP tools/list 条目格式。
fn descriptor_to_mcp_tool(desc: &ToolDescriptor) -> serde_json::Value {
    serde_json::json!({
        "name": desc.name,
        "description": desc.description,
        "inputSchema": desc.input_schema,
    })
}

fn main() {
    let router = build_router();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // 预取工具列表（tools/list 高频调用，router 内容不变后缓存）
    let tools_cache: Vec<serde_json::Value> = router
        .list()
        .iter()
        .map(descriptor_to_mcp_tool)
        .collect();

    eprintln!(
        "forge-mcp-server: serving {} tools over stdio",
        tools_cache.len()
    );

    // tokio runtime 用于异步工具调用
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("forge-mcp-server: failed to create tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned();

        match (method, id) {
            ("initialize", Some(id)) => {
                let r = serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "forge",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                writeln!(out, "{}", respond(&id, r)).unwrap();
                out.flush().unwrap();
            }
            ("notifications/initialized", None) => {
                // 握手确认通知，无需响应
            }
            ("tools/list", Some(id)) => {
                let r = serde_json::json!({ "tools": tools_cache });
                writeln!(out, "{}", respond(&id, r)).unwrap();
                out.flush().unwrap();
            }
            ("tools/call", Some(id)) => {
                let name = v
                    .pointer("/params/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let arguments = v
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                match router.route(name) {
                    Ok(tool) => {
                        let result = rt.block_on(tool.invoke(arguments));
                        match result {
                            Ok(val) => {
                                let text = match &val {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                let r = serde_json::json!({
                                    "content": [{ "type": "text", "text": text }]
                                });
                                writeln!(out, "{}", respond(&id, r)).unwrap();
                            }
                            Err(e) => {
                                writeln!(
                                    out,
                                    "{}",
                                    respond_error(&id, -32603, &format!("tool error: {e}"))
                                )
                                .unwrap();
                            }
                        }
                        out.flush().unwrap();
                    }
                    Err(_) => {
                        writeln!(
                            out,
                            "{}",
                            respond_error(&id, -32601, &format!("tool not found: {name}"))
                        )
                        .unwrap();
                        out.flush().unwrap();
                    }
                }
            }
            ("ping", Some(id)) => {
                // MCP 心跳
                writeln!(out, "{}", respond(&id, serde_json::json!({}))).unwrap();
                out.flush().unwrap();
            }
            (_, None) => { /* 其他通知：忽略 */ }
            (_, Some(id)) => {
                writeln!(
                    out,
                    "{}",
                    respond_error(&id, -32601, "method not found")
                )
                .unwrap();
                out.flush().unwrap();
            }
        }
    }
    // EOF：stdin 关闭后自然退出
}
