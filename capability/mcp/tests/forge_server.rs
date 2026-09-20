//! Forge MCP Server 集成测试：自环验证（client → forge-mcp-server → tool invoke）。
//!
//! 测试策略：
//! - 用 forge-mcp 的 McpClient 连 forge-mcp-server binary（CARGO_BIN_EXE 引用）
//! - 验证 initialize / tools/list / tools/call 全链路
//! - 验证 FORGE_TOOLS_BUILTIN 白名单生效
//! - 验证 工具不存在时返回 error
//!
//! 所有测试通过 env ISOLATION_DIR 隔离工作区，不触碰真实文件系统。

#![cfg(feature = "server-bin")]

use forge_mcp::client::McpClient;
use forge_mcp::config::McpServerConfig;
use std::collections::HashMap;
use std::path::PathBuf;

/// 构建 forge-mcp-server 的 McpServerConfig。
fn server_config(extra_env: Vec<(&str, &str)>) -> McpServerConfig {
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let mut env = HashMap::new();
    for (k, v) in extra_env {
        env.insert(k.to_string(), v.to_string());
    }
    McpServerConfig {
        name: "forge".into(),
        command: bin.into(),
        args: vec![],
        env,
    }
}

/// 临时工作区目录。
fn temp_workspace() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "forge-mcp-server-test-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[tokio::test]
async fn mcp_server_initialize_handshake() {
    let cfg = server_config(vec![("FORGE_WORKSPACE", "/tmp")]);
    let client = McpClient::connect(&cfg).await.unwrap();

    assert_eq!(client.server_info.name, "forge");
    // version 来自 CARGO_PKG_VERSION，非空即可
    assert!(!client.server_info.version.is_empty());

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_default_tools_list_has_5_base() {
    // 不设 FORGE_TOOLS_BUILTIN → 只有 5 个 BASE_TOOLS
    let ws = temp_workspace();
    let cfg = server_config(vec![("FORGE_WORKSPACE", ws.to_str().unwrap())]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 5, "default should have exactly 5 base tools");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"list_dir"));
    assert!(names.contains(&"edit_patch"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_builtin_whitelist_adds_extra_tools() {
    // 设 FORGE_TOOLS_BUILTIN=csv_parse,markdown_render → 5+2=7
    let ws = temp_workspace();
    let cfg = server_config(vec![
        ("FORGE_WORKSPACE", ws.to_str().unwrap()),
        ("FORGE_TOOLS_BUILTIN", "csv_parse,markdown_render"),
    ]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 7, "5 base + 2 whitelist = 7");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"csv_parse"));
    assert!(names.contains(&"markdown_render"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_builtin_whitelist_shell_rejected() {
    // 壳工具不能注册：pdf_parse 是壳工具
    let ws = temp_workspace();
    let cfg = server_config(vec![
        ("FORGE_WORKSPACE", ws.to_str().unwrap()),
        ("FORGE_TOOLS_BUILTIN", "pdf_parse,csv_parse"),
    ]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let tools = client.list_tools().await.unwrap();
    // pdf_parse 被拒，csv_parse 注册成功 → 5+1=6
    assert_eq!(tools.len(), 6);

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"pdf_parse"), "shell tool must be rejected");
    assert!(names.contains(&"csv_parse"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_echo_tool_call() {
    let ws = temp_workspace();
    let cfg = server_config(vec![("FORGE_WORKSPACE", ws.to_str().unwrap())]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let result = client
        .call_tool("echo", serde_json::json!({"msg": "hello"}))
        .await
        .unwrap();

    // call_tool 返回 MCP raw result: {"content":[{"type":"text","text":"..."}]}
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    // echo 返回 {"echo": <input>}
    assert_eq!(parsed["echo"]["msg"], "hello");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_csv_parse_tool_call() {
    let ws = temp_workspace();
    let cfg = server_config(vec![
        ("FORGE_WORKSPACE", ws.to_str().unwrap()),
        ("FORGE_TOOLS_BUILTIN", "csv_parse"),
    ]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let result = client
        .call_tool(
            "csv_parse",
            serde_json::json!({"text": "name,age\nAlice,30\nBob,25"}),
        )
        .await
        .unwrap();

    let text = result["content"][0]["text"]
        .as_str()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["headers"][0], "name");
    assert_eq!(parsed["headers"][1], "age");
    assert_eq!(parsed["row_count"], 2);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_tool_not_found_returns_error() {
    let ws = temp_workspace();
    let cfg = server_config(vec![("FORGE_WORKSPACE", ws.to_str().unwrap())]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let result = client.call_tool("nonexistent_tool", serde_json::json!({})).await;

    assert!(result.is_err(), "calling nonexistent tool must error");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_write_and_read_file_roundtrip() {
    let ws = temp_workspace();
    let cfg = server_config(vec![("FORGE_WORKSPACE", ws.to_str().unwrap())]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    // 先写文件
    let _write_result = client
        .call_tool(
            "write_file",
            serde_json::json!({
                "path": "mcp_test_file.txt",
                "content": "Hello from MCP!\nLine 2\n"
            }),
        )
        .await
        .unwrap();

    // 再读回来
    let read_result = client
        .call_tool(
            "read_file",
            serde_json::json!({"path": "mcp_test_file.txt"}),
        )
        .await
        .unwrap();

    // call_tool 返回 {"content":[{"type":"text","text":"..."}]}
    let text = read_result["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("Hello from MCP!"), "read content must match written");

    // 清理
    let _ = std::fs::remove_file(ws.join("mcp_test_file.txt"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_tool_call_with_empty_args() {
    let ws = temp_workspace();
    let cfg = server_config(vec![("FORGE_WORKSPACE", ws.to_str().unwrap())]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    // 用空参数调 echo（echo 不需要特定参数结构）
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .await
        .unwrap();

    let text = result["content"][0]["text"]
        .as_str()
        .unwrap();
    // echo 回显空对象 {"echo":{}}
    assert!(text.contains("echo"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_server_unknown_whitelist_ignored() {
    // 白名单中有未知工具名 → 不影响其他工具注册
    let ws = temp_workspace();
    let cfg = server_config(vec![
        ("FORGE_WORKSPACE", ws.to_str().unwrap()),
        ("FORGE_TOOLS_BUILTIN", "csv_parse,fake_tool,markdown_render"),
    ]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let tools = client.list_tools().await.unwrap();
    // fake_tool 未知 → 5+2=7（csv_parse + markdown_render 注册成功，fake_tool 跳过）
    assert_eq!(tools.len(), 7);

    client.shutdown().await.unwrap();
}
