//! MCP 适配器骨架。
//!
//! 第一阶段：校验 + 记录；不真正 spawn。
//! 第二阶段：进程生命周期 + JSON-RPC 握手。

use crate::config::McpServerConfig;
use forge_cap::{Capability, CapabilityKind, CapabilityStatus};
use forge_core::{CapabilityId, ForgeResult};
use forge_exec::PermissionLevel;

/// MCP 适配器。
pub struct McpAdapter {
    config: McpServerConfig,
}

impl McpAdapter {
    /// 构造即校验。
    pub fn new(config: McpServerConfig) -> ForgeResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// 返回注册用 Capability（kind=McpServer, status=Registered）。
    pub fn to_capability(&self) -> Capability {
        Capability {
            id: CapabilityId::new_capability_id(),
            name: self.config.name.clone(),
            kind: CapabilityKind::McpServer,
            version: "0.1.0".into(),
            entry: format!(
                "{} {}",
                self.config.command,
                self.config.args.join(" ")
            ),
            status: CapabilityStatus::Registered,
            permission: PermissionLevel::External,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> McpServerConfig {
        McpServerConfig {
            name: "test-mcp".into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            env: HashMap::new(),
        }
    }

    #[test]
    fn test_to_capability() {
        let adapter = McpAdapter::new(make_config()).unwrap();
        let cap = adapter.to_capability();
        assert_eq!(cap.name, "test-mcp");
        assert_eq!(cap.kind, CapabilityKind::McpServer);
        assert_eq!(cap.status, CapabilityStatus::Registered);
        assert_eq!(cap.permission, PermissionLevel::External);
    }

    #[test]
    fn test_empty_command_fails() {
        let mut config = make_config();
        config.command = "".into();
        assert!(McpAdapter::new(config).is_err());
    }
}
