//! forge-mcp：MCP 外部能力接入最小骨架。
//!
//! 第一阶段：配置、校验、进程管理骨架。
//! 完整协议实现（stdio 握手、list-tools、调用转发）属第二阶段。

pub mod adapter;
pub mod config;

pub use adapter::McpAdapter;
pub use config::McpServerConfig;
