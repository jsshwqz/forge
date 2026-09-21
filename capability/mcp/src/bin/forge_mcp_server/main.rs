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
//! 工具注册：启动时注册 BASE_TOOLS(5)（恒注册），内置白名单工具与编排工具
//! （MCP-002）由 FORGE_TOOLS_BUILTIN 白名单点名注册，缺省零回归。
//! 工具构造逻辑与 server/src/builtin_tools.rs 的 construct_tool 保持一致。
//!
//! 用法：
//!   forge-mcp-server
//!   FORGE_TOOLS_BUILTIN=csv_parse,markdown_render forge-mcp-server
//!   FORGE_TOOLS_BUILTIN=forge_task_create,forge_task_get,forge_task_list,forge_orchestrate \
//!     FORGE_WORKSPACE=/tmp/ws forge-mcp-server
//!
//! 被 Forge MCP 客户端消费时，client 侧的 FORGE_MCP_SERVERS 配置示例：
//!   FORGE_MCP_SERVERS='[{"name":"forge","command":"forge-mcp-server","args":[],"env":{}}]'
//!
//! 调用白名单（FORGE_MCP_ALLOWLIST）是 client 侧 mcp_tools.rs 的行为，
//! 不在本 binary 侧——binary 会列出所有已注册工具，是否允许调用由 client 决定。
//! MCP-002-B：本 binary 侧新增同名 env 的服务端调用闸 FORGE_MCP_ALLOWLIST：
//! 设置后仅白名单内工具可被 tools/call 调用，未设置 = 全部放行（与 MCP-001 兼容）。
//! 两处同名 env 语义区分：client 侧 = 接入闸（注册哪些桥接工具）；
//! binary 侧 = 服务端调用闸（tools/call 前置过滤）。

mod llm_wire;
mod orchestrate_tools;
mod planner;
mod worklog_tools;

use forge_exec::{Tool, ToolDescriptor, ToolRouter};
use forge_mcp::jsonrpc::PROTOCOL_VERSION;
use std::io::{BufRead, Write};
use std::sync::Arc;

use orchestrate_tools::{OrchestrateContext, ORCHESTRATE_TOOLS};
use worklog_tools::{construct_worklog_tool, WORKLOG_TOOLS};

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
/// 2. FORGE_TOOLS_BUILTIN 白名单工具按 env 注册（csv_parse, markdown_render 等；
///    编排工具 forge_* 也在白名单点名后注册，MCP-002）
/// 3. 壳工具（SHELL_TOOLS）一律跳过
async fn build_router() -> Arc<ToolRouter> {
    // 工作区根目录：FORGE_WORKSPACE 环境变量优先，缺省 "."（当前目录）。
    let workspace_root = std::path::PathBuf::from(
        std::env::var("FORGE_WORKSPACE").unwrap_or_else(|_| ".".into()),
    );
    let router = Arc::new(ToolRouter::new());
    let mut registered: Vec<&str> = Vec::new();
    let mut skipped: Vec<&str> = Vec::new();
    let mut rejected: Vec<&str> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();

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
        for name in builtin_raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if is_shell_tool(name) {
                rejected.push(name);
                continue;
            }
            if ORCHESTRATE_TOOLS.contains(&name) || WORKLOG_TOOLS.contains(&name) {
                // MCP-002/003：编排/台账工具由下方独立段缺省注册，此处跳过
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

    // MCP-002/003/005：编排+台账工具缺省全注册（任何 agent 一启动即用，
    // 不限任务、不限白名单点名；worklog 工具根由 FORGE_PROJECT_ROOT 探测）
    let ctx = Arc::new(
        OrchestrateContext::new().await.unwrap_or_else(|e| {
            eprintln!("forge-mcp-server: orchestrate context unavailable: {e}");
            std::process::exit(1);
        }),
    );
    for name in ORCHESTRATE_TOOLS {
        if router.route(name).is_ok() {
            skipped.push(name);
            continue;
        }
        match orchestrate_tools::construct_orchestrate_tool(name, &ctx) {
            Some(tool) => match router.register(tool) {
                Ok(()) => registered.push(name),
                Err(_) => skipped.push(name),
            },
            None => unknown.push(name),
        }
    }
    for name in WORKLOG_TOOLS {
        if router.route(name).is_ok() {
            skipped.push(name);
            continue;
        }
        match construct_worklog_tool(name) {
            Some(tool) => match router.register(tool) {
                Ok(()) => registered.push(name),
                Err(_) => skipped.push(name),
            },
            None => unknown.push(name),
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


// ── IMPROVE-8: .env 自动加载 ──
//
// 从工作区根目录的 .env 文件读取 KEY=VALUE 行并注入 std::env。
// 已有的环境变量优先（不覆盖），与 docker-compose --env-file 语义一致。
// 查找顺序: FORGE_WORKSPACE/.env → 当前目录 .env → 上级目录 .env
fn load_dotenv() {
    let candidates: Vec<std::path::PathBuf> = {
        let mut v = Vec::new();
        if let Ok(ws) = std::env::var("FORGE_WORKSPACE") {
            v.push(std::path::PathBuf::from(&ws).join(".env"));
        }
        v.push(std::path::PathBuf::from(".env"));
        // 上溯两级查找（forge-mcp-server 可能从 target/debug 启动）
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(p) = cwd.parent() {
                v.push(p.join(".env"));
                if let Some(pp) = p.parent() {
                    v.push(pp.join(".env"));
                }
            }
        }
        v
    };

    for path in &candidates {
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut loaded = 0usize;
        for line in content.lines() {
            let line = line.trim();
            // 跳过空行和注释
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // 解析 KEY=VALUE
            if let Some(eq_idx) = line.find('=') {
                let key = line[..eq_idx].trim();
                let val = line[eq_idx + 1..].trim();
                // 去掉两端引号
                let val = val
                    .strip_prefix('"').and_then(|v| v.strip_suffix('"'))
                    .or_else(|| val.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(val);
                if !key.is_empty() {
                    // 不覆盖已有环境变量
                    if std::env::var_os(key).is_none() {
                        std::env::set_var(key, val);
                        loaded += 1;
                    }
                }
            }
        }
        if loaded > 0 {
            eprintln!(
                "forge-mcp-server: loaded {loaded} env vars from {}",
                path.display()
            );
        }
        break; // 只读第一个找到的 .env
    }
}


fn main() {
    // IMPROVE-8: 启动时自动加载 .env（不覆盖已有环境变量）
    load_dotenv();

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("forge-mcp-server: failed to create tokio runtime: {e}");
            std::process::exit(1);
        }
    };
    let router = rt.block_on(build_router());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // 预取工具列表（tools/list 高频调用，router 内容不变后缓存）
    let tools_cache: Vec<serde_json::Value> = router
        .list()
        .iter()
        .map(descriptor_to_mcp_tool)
        .collect();

    // MCP-002-B：服务端调用闸（tools/call 前置过滤）。
    // env FORGE_MCP_ALLOWLIST（逗号分隔）；未设置 = 全部放行。
    let call_allowlist: Option<Vec<String>> = std::env::var("FORGE_MCP_ALLOWLIST")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        });

    eprintln!(
        "forge-mcp-server: serving {} tools over stdio",
        tools_cache.len()
    );

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
                // MCP-002-B：白名单外拒绝（未设置 allowlist = 全放行）
                if let Some(allowed) = &call_allowlist {
                    if !allowed.iter().any(|a| a == name) {
                        writeln!(
                            out,
                            "{}",
                            respond_error(
                                &id,
                                -32601,
                                &format!("allowlist rejected: {name}")
                            )
                        )
                        .unwrap();
                        out.flush().unwrap();
                        continue;
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::load_dotenv;
    use std::io::Write;

    #[test]
    fn dotenv_loads_from_file() {
        let tmp = tempfile::tempdir().unwrap();
        let env_path = tmp.path().join(".env");
        let mut f = std::fs::File::create(&env_path).unwrap();
        writeln!(f, "# comment").unwrap();
        writeln!(f, "FORGE_TEST_KEY_1=value1").unwrap();
        writeln!(f, r#"FORGE_TEST_KEY_2="quoted value""#).unwrap();
        writeln!(f).unwrap();
        writeln!(f, "FORGE_TEST_KEY_3=unquoted").unwrap();
        drop(f);

        // 清理可能残留的 env
        std::env::remove_var("FORGE_TEST_KEY_1");
        std::env::remove_var("FORGE_TEST_KEY_2");
        std::env::remove_var("FORGE_TEST_KEY_3");

        // 临时切换 cwd 到 tempdir 并调用
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        // 清除 FORGE_WORKSPACE 以免干扰
        std::env::remove_var("FORGE_WORKSPACE");
        load_dotenv();
        std::env::set_current_dir(orig_cwd).unwrap();

        assert_eq!(std::env::var("FORGE_TEST_KEY_1").unwrap(), "value1");
        assert_eq!(std::env::var("FORGE_TEST_KEY_2").unwrap(), "quoted value");
        assert_eq!(std::env::var("FORGE_TEST_KEY_3").unwrap(), "unquoted");

        // 清理
        std::env::remove_var("FORGE_TEST_KEY_1");
        std::env::remove_var("FORGE_TEST_KEY_2");
        std::env::remove_var("FORGE_TEST_KEY_3");
    }

    #[test]
    fn dotenv_does_not_override_existing() {
        // 已有的环境变量不应被 .env 覆盖
        std::env::set_var("FORGE_TEST_OVERRIDE", "original");
        let tmp = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(tmp.path().join(".env")).unwrap();
        writeln!(f, "FORGE_TEST_OVERRIDE=from_file").unwrap();
        drop(f);

        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        std::env::remove_var("FORGE_WORKSPACE");
        load_dotenv();
        std::env::set_current_dir(orig_cwd).unwrap();

        assert_eq!(std::env::var("FORGE_TEST_OVERRIDE").unwrap(), "original");
        std::env::remove_var("FORGE_TEST_OVERRIDE");
    }
}
