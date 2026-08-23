//! forge-mcp：MCP 外部能力接入（第二阶段：完整 stdio 客户端协议）。
//!
//! - `McpServerConfig` / `McpAdapter`：配置模型与能力注册骨架
//! - `McpClient`：spawn 子进程 + JSON-RPC 握手 + tools/list + tools/call（B-03）
//!
//! 协议版本：2024-11-05；传输：行分隔 JSON-RPC 2.0 over stdio。

pub mod adapter;
pub mod client;
pub mod config;
pub mod jsonrpc;

pub use adapter::McpAdapter;
pub use client::{McpClient, McpTool, ServerInfo};
pub use config::McpServerConfig;
pub use jsonrpc::PROTOCOL_VERSION;
