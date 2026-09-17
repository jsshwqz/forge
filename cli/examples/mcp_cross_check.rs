//! 交叉验证：用 forge-mcp 的 McpClient 连 `forge mcp-server`（新实现），
//! 与 AionUI 桥（rmcp）互为独立客户端，确认新 server 的 MCP 合规性。
use forge_mcp::{McpClient, McpServerConfig};
use std::collections::HashMap;

#[tokio::main]
async fn main() {
    let exe = std::env::args().nth(1).unwrap_or_else(|| {
        let p = std::env::current_dir().unwrap().join("target/debug/forge.exe");
        p.to_string_lossy().into_owned()
    });
    let cfg = McpServerConfig {
        name: "forge".into(),
        command: exe,
        args: vec!["mcp-server".into()],
        env: HashMap::new(),
    };
    let mut client = McpClient::connect(&cfg).await.expect("connect + initialize 失败");
    let info = client.server_info.clone();
    println!("server_info: name={} version={}", info.name, info.version);
    let tools = client.list_tools().await.expect("tools/list 失败");
    println!(
        "tools: {}",
        tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let call = client
        .call_tool("echo", serde_json::json!({ "text": "cross-check via forge-mcp client" }))
        .await
        .expect("tools/call 失败");
    println!("echo result: {}", call.get("content").unwrap());
    client
        .shutdown()
        .await
        .expect("shutdown 失败");
    println!("CROSS-CHECK PASS: forge-mcp McpClient 连新 forge mcp-server 完成 echo 往返");
}
