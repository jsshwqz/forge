//! PH2-003 集成测试：通过 mock-mcp-server 二进制验证完整 stdio 链路。
//!
//! 完全离线：CARGO_BIN_EXE 由 cargo 提供，无需任何外部服务。

use forge_mcp::{McpClient, McpServerConfig};
use std::collections::HashMap;

fn mock_config() -> McpServerConfig {
    McpServerConfig {
        name: "mock".into(),
        command: env!("CARGO_BIN_EXE_mock-mcp-server").into(),
        args: vec![],
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn handshake_list_call_shutdown_full_flow() {
    let mut c = McpClient::connect(&mock_config()).await.unwrap();

    // 握手结果
    assert_eq!(c.server_info.name, "mock");
    assert_eq!(c.server_info.version, "0.1.0");

    // tools/list
    let tools = c.list_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description.as_deref(), Some("Echo back arguments as text"));
    assert!(tools[0].input_schema.is_some());

    // tools/call
    let out = c.call_tool("echo", serde_json::json!({"msg": "hi"})).await.unwrap();
    let text = out
        .pointer("/content/0/text")
        .and_then(|t| t.as_str())
        .expect("content[0].text should exist");
    assert_eq!(text, r#"{"msg":"hi"}"#);

    // 优雅关闭
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_method_maps_to_error() {
    let mut c = McpClient::connect(&mock_config()).await.unwrap();
    let err = c.call_tool("no_such_tool", serde_json::json!({})).await;
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("-32601") || msg.contains("not found"),
        "unexpected error: {msg}"
    );
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_command_fails_fast() {
    let mut cfg = mock_config();
    cfg.command = String::new();
    assert!(McpClient::connect(&cfg).await.is_err());
}
