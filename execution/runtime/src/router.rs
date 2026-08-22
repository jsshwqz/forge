//! 工具路由器与内置工具。

use crate::permission_level::PermissionLevel;
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 工具描述符。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// 工具名称。
    pub name: String,
    /// 工具描述。
    pub description: String,
    /// 输入 JSON Schema（第一阶段仅存储不强校验）。
    pub input_schema: serde_json::Value,
    /// 所需权限级别。
    pub permission: PermissionLevel,
}

/// 工具 trait。实现自身不做权限判断（检查在 EXEC-003 策略层）。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 获取工具描述符。
    fn descriptor(&self) -> &ToolDescriptor;

    /// 调用工具。
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value>;
}

/// 工具路由器。
///
/// 路由表内部用 `RwLock<HashMap<String, Arc<dyn Tool>>>`。
/// `route` 返回 `Arc<dyn Tool>` 以保证跨锁引用的内存安全。
pub struct ToolRouter {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl ToolRouter {
    /// 创建空路由器。
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }

    /// 注册工具。重名返回 `InvalidState`。
    pub fn register(&self, tool: Box<dyn Tool>) -> ForgeResult<()> {
        let name = tool.descriptor().name.clone();
        let mut guard = self
            .tools
            .write()
            .map_err(|e| ForgeError::InvalidState(format!("router lock poisoned: {}", e)))?;
        if guard.contains_key(&name) {
            return Err(ForgeError::InvalidState(format!(
                "tool already registered: {}",
                name
            )));
        }
        guard.insert(name, Arc::from(tool));
        Ok(())
    }

    /// 路由到指定工具。未注册返回 `NotFound`。
    pub fn route(&self, name: &str) -> ForgeResult<Arc<dyn Tool>> {
        let guard = self
            .tools
            .read()
            .map_err(|e| ForgeError::InvalidState(format!("router lock poisoned: {}", e)))?;
        guard
            .get(name)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("tool: {}", name)))
    }

    /// 列出所有工具描述符。
    pub fn list(&self) -> Vec<ToolDescriptor> {
        let guard = self.tools.read().unwrap_or_else(|e| e.into_inner());
        guard.values().map(|t| t.descriptor().clone()).collect()
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// 内置 Echo 工具：原样返回 `{"echo": input}`，permission = ReadOnly。
pub struct EchoTool {
    descriptor: ToolDescriptor,
}

impl Default for EchoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoTool {
    /// 创建 EchoTool。
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "echo".into(),
                description: "Echoes back the input".into(),
                input_schema: serde_json::json!({"type": "object"}),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(serde_json::json!({"echo": input}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_route() {
        let router = ToolRouter::new();
        router.register(Box::new(EchoTool::new())).unwrap();

        let tool = router.route("echo").unwrap();
        let result = tool.invoke(serde_json::json!({"msg": "hello"})).await.unwrap();
        assert_eq!(result["echo"]["msg"], "hello");
    }

    #[tokio::test]
    async fn test_duplicate_register() {
        let router = ToolRouter::new();
        router.register(Box::new(EchoTool::new())).unwrap();
        let result = router.register(Box::new(EchoTool::new()));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_route_not_found() {
        let router = ToolRouter::new();
        let result = router.route("nonexistent");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list() {
        let router = ToolRouter::new();
        router.register(Box::new(EchoTool::new())).unwrap();
        let list = router.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "echo");
        assert_eq!(list[0].permission, PermissionLevel::ReadOnly);
    }

    #[tokio::test]
    async fn test_echo_tool() {
        let tool = EchoTool::new();
        let input = serde_json::json!({"key": "value", "num": 42});
        let result = tool.invoke(input.clone()).await.unwrap();
        assert_eq!(result, serde_json::json!({"echo": input}));
    }
}
