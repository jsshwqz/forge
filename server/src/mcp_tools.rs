//! V7 ORCH-101c：MCP 工具源接入（build_v70a.md 契约）。
//!
//! R1 白名单注册制：McpAdapter.to_capability() 形态的能力经 ToolRouter 注册，
//! 白名单外一律不注册；R2 无声明的 MCP 工具默认 External（进沙箱策略链，101b）；
//! R3 未配置零开销（不建连接）；R4 e2e 复用仓库既有 mock-mcp-server 二进制。
//!
//! 实现注记：桥接工具在 invoke 时按需 connect→call→shutdown（每次调用独立进程）。
//! 连接复用（AppState 缓存客户端）属性能优化不属契约，留待运营数据驱动立项。

use forge_exec::{PermissionLevel, Tool, ToolDescriptor, ToolRouter};
use forge_mcp::{McpAdapter, McpClient, McpServerConfig, McpTool};
use forge_core::{ForgeError, ForgeResult};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

/// 环境配置缓存（R3：未设置 → 空表，零开销）。
fn configs_cache() -> &'static Vec<McpServerConfig> {
    static CACHE: OnceLock<Vec<McpServerConfig>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let raw = std::env::var("FORGE_MCP_SERVERS").unwrap_or_default();
        if raw.trim().is_empty() {
            return Vec::new();
        }
        serde_json::from_str::<Vec<McpServerConfig>>(&raw).unwrap_or_else(|e| {
            eprintln!("mcp_tools: FORGE_MCP_SERVERS parse failed ({e}), MCP disabled");
            Vec::new()
        })
    })
}

/// 读取 MCP 服务器配置（env `FORGE_MCP_SERVERS`，JSON 数组）。
pub fn configs_from_env() -> Vec<McpServerConfig> {
    configs_cache().clone()
}

/// 允许清单：env `FORGE_MCP_ALLOWLIST`（逗号分隔）；未设置 → None = **禁用**（不注册任何
/// MCP 工具，最安全口径）。调用方在 None 时跳过注册。
pub fn allowlist_from_env() -> Option<HashSet<String>> {
    let raw = std::env::var("FORGE_MCP_ALLOWLIST").unwrap_or_default();
    if raw.trim().is_empty() {
        return None;
    }
    Some(raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

/// 桥接后的工具全名（冻结：`mcp_<server>_<tool>`）。
pub fn bridged_name(server: &str, tool: &str) -> String {
    format!("mcp_{server}_{tool}")
}

/// MCP 桥接工具：invoke 时 connect → tools/call → shutdown。
struct McpBridgeTool {
    cfg: McpServerConfig,
    tool: McpTool,
    desc: ToolDescriptor,
}

impl McpBridgeTool {
    fn new(cfg: McpServerConfig, tool: McpTool) -> Self {
        let full = bridged_name(&cfg.name, &tool.name);
        let desc = ToolDescriptor {
            name: full,
            description: format!(
                "MCP tool from server '{}': {}",
                cfg.name,
                tool.description.clone().unwrap_or_default()
            ),
            input_schema: tool.input_schema.clone().unwrap_or_else(|| {
                serde_json::json!({ "type": "object" })
            }),
            permission: PermissionLevel::External, // R2：无声明默认 External（进沙箱链）
        };
        Self { cfg, tool, desc }
    }
}

#[async_trait::async_trait]
impl Tool for McpBridgeTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.desc
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let mut client = McpClient::connect(&self.cfg).await?;
        let out = client.call_tool(&self.tool.name, input).await;
        let _ = client.shutdown().await;
        out
    }
}

/// 发现并注册 MCP 工具（契约冻结签名）。
///
/// 对每个 config：McpAdapter 校验 → connect → list_tools → shutdown →
/// 白名单过滤后以 `mcp_<server>_<tool>` 注册桥接工具。白名单同时匹配
/// raw 工具名与全名（任一命中即注册）。返回注册数。
pub async fn register_mcp_tools(
    router: &ToolRouter,
    configs: &[McpServerConfig],
    whitelist: &HashSet<String>,
) -> ForgeResult<usize> {
    let mut registered = 0usize;
    for cfg in configs {
        // R1：McpAdapter 构造即校验，Capability 形态可供 registry 面（此处仅校验）
        McpAdapter::new(cfg.clone())?;
        let mut client = McpClient::connect(cfg).await?;
        let tools = match client.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                let _ = client.shutdown().await;
                return Err(ForgeError::InvalidState(format!(
                    "mcp_tools: list_tools({}) failed: {e}",
                    cfg.name
                )));
            }
        };
        let _ = client.shutdown().await;
        for tool in tools {
            let full = bridged_name(&cfg.name, &tool.name);
            if !whitelist.contains(&tool.name) && !whitelist.contains(&full) {
                continue; // R1：白名单外一律不注册
            }
            router.register(Box::new(McpBridgeTool::new(cfg.clone(), tool)))?;
            registered += 1;
        }
    }
    Ok(registered)
}
