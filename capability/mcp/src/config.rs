//! MCP 服务器配置模型。

use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP 服务器配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 服务器名称。
    pub name: String,
    /// 启动命令。
    pub command: String,
    /// 命令参数。
    pub args: Vec<String>,
    /// 环境变量。
    pub env: HashMap<String, String>,
}

impl McpServerConfig {
    /// 校验配置。name/command 非空；重名校验由调用方结合 registry 完成。
    pub fn validate(&self) -> ForgeResult<()> {
        if self.name.is_empty() {
            return Err(ForgeError::InvalidState(
                "McpServerConfig: 'name' must not be empty".into(),
            ));
        }
        if self.command.is_empty() {
            return Err(ForgeError::InvalidState(
                "McpServerConfig: 'command' must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_config() {
        let config = McpServerConfig {
            name: "test-server".into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            env: HashMap::new(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_empty_command() {
        let config = McpServerConfig {
            name: "test".into(),
            command: "".into(),
            args: vec![],
            env: HashMap::new(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_empty_name() {
        let config = McpServerConfig {
            name: "".into(),
            command: "node".into(),
            args: vec![],
            env: HashMap::new(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = McpServerConfig {
            name: "test".into(),
            command: "node".into(),
            args: vec!["a".into(), "b".into()],
            env: HashMap::from([("KEY".into(), "VALUE".into())]),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.name, back.name);
        assert_eq!(config.command, back.command);
        assert_eq!(config.args, back.args);
    }
}
