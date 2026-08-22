//! 动作分发器桥接：将 AgentAction 转发到 ExecutionEngine。

use crate::engine::{ExecutionEngine, ExecutionRequest};
use async_trait::async_trait;
use forge_agent::{ActionDispatcher, AgentAction};
use forge_core::{ExecutionId, ForgeError, ForgeResult, SessionId};
use std::sync::Arc;

/// 引擎分发器：将 Agent 的 CallTool 动作桥接到 ExecutionEngine。
pub struct EngineDispatcher {
    engine: Arc<ExecutionEngine>,
    session_id: SessionId,
}

impl EngineDispatcher {
    /// 创建分发器。
    pub fn new(engine: Arc<ExecutionEngine>, session_id: SessionId) -> Self {
        Self { engine, session_id }
    }
}

#[async_trait]
impl ActionDispatcher for EngineDispatcher {
    async fn dispatch(&self, action: &AgentAction) -> ForgeResult<serde_json::Value> {
        match action {
            AgentAction::CallTool { tool, input } => {
                let req = ExecutionRequest {
                    execution_id: ExecutionId::new_execution_id(),
                    session_id: self.session_id.clone(),
                    step_id: String::new(),
                    tool: tool.clone(),
                    input: input.clone(),
                };
                let result = self.engine.execute(req).await?;
                Ok(result.output)
            }
            _ => Err(ForgeError::InvalidState(
                "dispatcher only accepts CallTool actions".into(),
            )),
        }
    }
}
