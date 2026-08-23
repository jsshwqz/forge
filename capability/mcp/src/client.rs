//! MCP stdio 客户端：进程生命周期 + JSON-RPC 握手 + tools/list + tools/call。
//!
//! 落实施工包 B-03（第二阶段）：stdio 握手、list-tools、调用转发。
//!
//! 协议：行分隔 JSON-RPC 2.0（2024-11-05）。
//! - connect：spawn 子进程 → `initialize` → 校验 serverInfo → `notifications/initialized`
//! - request：自增 id，循环读行直到 id 匹配；服务端通知/其他请求按规范忽略
//! - 超时：单请求 10 秒；shutdown 先关 stdin 优雅退出，3 秒未退则 kill

use crate::config::McpServerConfig;
use crate::jsonrpc::{self, Incoming, JsonRpcNotification, JsonRpcRequest};
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// 单请求超时。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 服务端信息（initialize 响应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// 服务端暴露的工具（tools/list 条目）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

/// MCP stdio 客户端。
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    /// initialize 握手得到的服务端信息。
    pub server_info: ServerInfo,
}

impl McpClient {
    /// 启动子进程并完成 initialize 握手。
    pub async fn connect(cfg: &McpServerConfig) -> ForgeResult<Self> {
        cfg.validate()?;
        let mut child = tokio::process::Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ForgeError::InvalidState("mcp: child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ForgeError::InvalidState("mcp: child stdout unavailable".into()))?;

        let mut client = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 0,
            server_info: ServerInfo {
                name: String::new(),
                version: String::new(),
            },
        };

        // initialize 握手
        let params = serde_json::json!({
            "protocolVersion": jsonrpc::PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "forge-mcp", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = client.request("initialize", Some(params)).await?;
        let info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        client.server_info = serde_json::from_value(info)
            .unwrap_or(ServerInfo { name: "unknown".into(), version: String::new() });

        // initialized 通知（无 id）
        let note = JsonRpcNotification { jsonrpc: "2.0", method: "notifications/initialized" };
        let note_line = serde_json::to_string(&note)
            .map_err(|e| ForgeError::InvalidState(format!("mcp: serialize failed: {e}")))?;
        client.send_line(&note_line).await?;
        Ok(client)
    }

    async fn send_line(&mut self, s: &str) -> ForgeResult<()> {
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// 发送请求并等待 id 匹配的响应；服务端通知/其他请求忽略。
    async fn request(&mut self, method: &str, params: Option<Value>) -> ForgeResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest { jsonrpc: "2.0", id, method, params: params.as_ref() };
        let line = serde_json::to_string(&req)
            .map_err(|e| ForgeError::InvalidState(format!("mcp: serialize failed: {e}")))?;
        self.send_line(&line).await?;

        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ForgeError::InvalidState(format!("mcp request timed out: {method}")));
            }
            let mut line = String::new();
            let n = tokio::time::timeout(remaining, self.reader.read_line(&mut line))
                .await
                .map_err(|_| ForgeError::InvalidState(format!("mcp request timed out: {method}")))??;
            if n == 0 {
                return Err(ForgeError::InvalidState("mcp: server closed output".into()));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match jsonrpc::parse_incoming(trimmed) {
                Some(Incoming::Response { id: rid, result, error }) if rid == id => {
                    if let Some(err) = error {
                        return Err(ForgeError::InvalidState(format!(
                            "mcp error {}: {}",
                            err.code, err.message
                        )));
                    }
                    return Ok(result.unwrap_or(Value::Null));
                }
                _ => continue,
            }
        }
    }

    /// 列出服务端工具（tools/list）。
    pub async fn list_tools(&mut self) -> ForgeResult<Vec<McpTool>> {
        let result = self.request("tools/list", None).await?;
        let tools = result.get("tools").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(tools)
            .map_err(|e| ForgeError::InvalidState(format!("mcp: bad tools payload: {e}")))
    }

    /// 调用服务端工具（tools/call），返回原始 result（content 数组等）。
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> ForgeResult<Value> {
        self.request(
            "tools/call",
            Some(serde_json::json!({ "name": name, "arguments": arguments })),
        )
        .await
    }

    /// 优雅关闭：关 stdin → 等待 3 秒 → 超时 kill。
    pub async fn shutdown(self) -> ForgeResult<()> {
        let Self { mut stdin, mut child, .. } = self;
        let _ = stdin.shutdown().await;
        drop(stdin);
        match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
            Ok(status) => {
                let _ = status?;
                Ok(())
            }
            Err(_) => {
                let _ = child.kill().await;
                Ok(())
            }
        }
    }
}

/// 供外部构造配置的便捷函数（保持 env 类型一致）。
pub fn empty_env() -> HashMap<String, String> {
    HashMap::new()
}
