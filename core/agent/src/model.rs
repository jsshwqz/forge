//! Agent 模型定义。

use async_trait::async_trait;
use forge_core::{AgentId, ForgeResult, SessionId};
use serde::{Deserialize, Serialize};

/// Agent 角色枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    Architect,
    Builder,
    Tester,
    Reviewer,
    Orchestrator,
}

/// Agent 配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent ID。
    pub id: AgentId,
    /// 名称。
    pub name: String,
    /// 角色。
    pub role: AgentRole,
    /// 最大回合数，默认 50。
    pub max_turns: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: AgentId::new_agent_id(),
            name: "unnamed".into(),
            role: AgentRole::Builder,
            max_turns: 50,
        }
    }
}

/// 回合输入。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnInput {
    /// 会话 ID。
    pub session_id: SessionId,
    /// 回合编号，从 1 开始。
    pub turn: u32,
    /// 历史记录。
    pub history: Vec<serde_json::Value>,
    /// 上一动作的观察结果。
    pub observation: Option<serde_json::Value>,
}

/// Agent 动作。第一阶段动作空间封闭，不允许新增变体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgentAction {
    /// 回复文本。
    Reply(String),
    /// 调用工具。
    CallTool {
        /// 工具名称。
        tool: String,
        /// 工具输入。
        input: serde_json::Value,
    },
    /// 完成并给出结果。
    Finish(AgentOutcome),
    /// 中止并给出原因。
    Abort(String),
}

/// Agent 执行结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentOutcome {
    Success,
    Failed,
    NeedsHuman,
}

/// Agent trait。实现必须是无副作用声明式的：副作用由调度方执行。
#[async_trait]
pub trait Agent: Send + Sync {
    /// 获取配置。
    fn config(&self) -> &AgentConfig;

    /// 执行一个回合的决策。
    async fn act(&self, input: &TurnInput) -> ForgeResult<AgentAction>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试夹具：按预置脚本返回动作。
    struct ScriptedAgent {
        config: AgentConfig,
        script: tokio::sync::Mutex<std::collections::VecDeque<AgentAction>>,
    }

    impl ScriptedAgent {
        fn new(actions: Vec<AgentAction>) -> Self {
            Self {
                config: AgentConfig::default(),
                script: tokio::sync::Mutex::new(actions.into()),
            }
        }
    }

    #[async_trait]
    impl Agent for ScriptedAgent {
        fn config(&self) -> &AgentConfig {
            &self.config
        }

        async fn act(&self, _input: &TurnInput) -> ForgeResult<AgentAction> {
            let mut guard = self.script.lock().await;
            guard
                .pop_front()
                .ok_or_else(|| forge_core::ForgeError::InvalidState("script exhausted".into()))
        }
    }

    #[tokio::test]
    async fn test_scripted_agent_object_safe() {
        let agent: Box<dyn Agent> = Box::new(ScriptedAgent::new(vec![
            AgentAction::Reply("hello".into()),
            AgentAction::Finish(AgentOutcome::Success),
        ]));

        let input = TurnInput {
            session_id: SessionId::new_session_id(),
            turn: 1,
            history: vec![],
            observation: None,
        };

        let action1 = agent.act(&input).await.unwrap();
        assert_eq!(action1, AgentAction::Reply("hello".into()));

        let action2 = agent.act(&input).await.unwrap();
        assert_eq!(action2, AgentAction::Finish(AgentOutcome::Success));
    }

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.max_turns, 50);
        assert_eq!(config.role, AgentRole::Builder);
    }

    #[test]
    fn test_agent_action_equality() {
        let a1 = AgentAction::Reply("hello".into());
        let a2 = AgentAction::Reply("hello".into());
        assert_eq!(a1, a2);

        let a3 = AgentAction::CallTool {
            tool: "echo".into(),
            input: serde_json::json!({}),
        };
        let a4 = AgentAction::CallTool {
            tool: "echo".into(),
            input: serde_json::json!({}),
        };
        assert_eq!(a3, a4);
        assert_ne!(a1, a3);
    }

    #[test]
    fn test_agent_action_serde() {
        let action = AgentAction::CallTool {
            tool: "echo".into(),
            input: serde_json::json!({"msg": "hi"}),
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: AgentAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, back);
    }
}
