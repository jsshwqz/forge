//! LlmAgent：用真实模型驱动 Agent trait（B-05 第二阶段落地）。
//!
//! 设计要点：
//! - 后端抽象 [`LlmBackend`] 可注入 mock，单测离线；生产用 [`crate::LlmClient`]
//! - act() 无状态映射：system(角色) + 最近观察摘要 + 用户指令 → Reply(content)
//!   （多轮任务编排属后续编排层；循环/上限护栏由 TurnEngine 兜底）
//! - 模型自动发现：connect 时 list_models → pick_default_model

use crate::{pick_model_with_prefs, ChatMessage, LlmBackend, OFFICIAL_MODEL_PREFS};
use async_trait::async_trait;
use forge_agent::{
    Agent, AgentAction, AgentConfig, AgentRole, TurnInput,
};
use forge_core::ForgeResult;

/// 基于真实模型的 Agent。
pub struct LlmAgent<B: LlmBackend> {
    config: AgentConfig,
    backend: B,
    model: String,
    /// 发给模型的系统提示词。
    pub system_prompt: String,
}

impl<B: LlmBackend> LlmAgent<B> {
    /// 显式指定模型构造。
    pub fn new(config: AgentConfig, backend: B, model: impl Into<String>) -> Self {
        let role = config.role;
        Self {
            system_prompt: default_system_prompt(role),
            config,
            backend,
            model: model.into(),
        }
    }

    /// 自动发现模型后构造。
    pub async fn connect(
        config: AgentConfig,
        backend: B,
        model_override: Option<String>,
    ) -> forge_core::ForgeResult<Self> {
        let ids = backend.list_models().await?;
        let model = match model_override {
            Some(m) => m,
            None => pick_model_with_prefs(&ids, OFFICIAL_MODEL_PREFS)?,
        };
        Ok(Self::new(config, backend, model))
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

fn default_system_prompt(role: AgentRole) -> String {
    format!(
        "You are the {role:?} agent of AionForge 2.0. \
         Respond concisely and concretely in the user's language."
    )
}

/// 观察值截断上限（避免超长 payload 打爆上下文）。
const OBSERVATION_MAX_CHARS: usize = 800;

fn observation_to_text(o: &serde_json::Value) -> String {
    let s = o.to_string();
    if s.len() > OBSERVATION_MAX_CHARS {
        format!("{}…", s.chars().take(OBSERVATION_MAX_CHARS).collect::<String>())
    } else {
        s
    }
}

#[async_trait]
impl<B: LlmBackend> Agent for LlmAgent<B> {
    fn config(&self) -> &AgentConfig {
        &self.config
    }

    async fn act(&self, input: &TurnInput) -> ForgeResult<AgentAction> {
        let mut messages = vec![ChatMessage::system(self.system_prompt.clone())];

        // 最近的历史观察作为上下文（最多取 4 条，倒序截断保最近）
        for h in input.history.iter().rev().take(4).rev() {
            messages.push(ChatMessage::assistant(format!(
                "Previous observation: {}",
                observation_to_text(h)
            )));
        }

        // 当前指令
        let user_text = match &input.observation {
            Some(o) => format!(
                "Turn {}.\nLatest observation:\n{}\n\nPlease provide your next action.",
                input.turn,
                observation_to_text(o)
            ),
            None => format!(
                "Turn {}.\nTask started. Please provide your first step.",
                input.turn
            ),
        };
        messages.push(ChatMessage::user(user_text));

        let content = self.backend.chat(&self.model, &messages).await?;
        Ok(AgentAction::Reply(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_agent::AgentOutcome;
    use forge_core::SessionId;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockBackend {
        responses: Mutex<Vec<String>>,
        models: Vec<String>,
        last_model: Mutex<Option<String>>,
        last_messages: Mutex<Vec<ChatMessage>>,
    }

    impl MockBackend {
        fn new(reply: &str) -> Self {
            Self {
                responses: Mutex::new(vec![reply.to_string()]),
                models: vec!["mock-chat-1".into(), "other".into()],
                last_model: Mutex::new(None),
                last_messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl LlmBackend for MockBackend {
        async fn list_models(&self) -> ForgeResult<Vec<String>> {
            Ok(self.models.clone())
        }
        async fn chat(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String> {
            *self.last_model.lock().unwrap() = Some(model.to_string());
            *self.last_messages.lock().unwrap() = messages.to_vec();
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| forge_core::ForgeError::InvalidState("no scripted reply".into()))
        }
    }

    fn turn_input(observation: Option<serde_json::Value>, history: Vec<serde_json::Value>) -> TurnInput {
        TurnInput { session_id: SessionId::new_session_id(), turn: 1, history, observation }
    }

    #[tokio::test]
    async fn connect_auto_discovers_chat_model() {
        let backend = MockBackend::new("x");
        let agent = LlmAgent::connect(AgentConfig::default(), backend, None)
            .await
            .unwrap();
        assert_eq!(agent.model(), "mock-chat-1");
    }

    #[tokio::test]
    async fn connect_model_override_wins() {
        let backend = MockBackend::new("x");
        let agent =
            LlmAgent::connect(AgentConfig::default(), backend, Some("forced".into()))
                .await
                .unwrap();
        assert_eq!(agent.model(), "forced");
    }

    #[tokio::test]
    async fn act_maps_reply_and_includes_observation() {
        let backend = MockBackend::new("do step 1");
        let mut agent = LlmAgent::new(AgentConfig::default(), backend, "m");
        agent.system_prompt = "SYS".into();

        let obs = json!({"echo": {"msg": "hello"}});
        let action = agent.act(&turn_input(Some(obs.clone()), vec![])).await.unwrap();

        match action {
            AgentAction::Reply(text) => assert_eq!(text, "do step 1"),
            other => panic!("expected Reply, got {other:?}"),
        }

        // 校验发给后端的消息结构
        let b_msgs = agent.backend.last_messages.lock().unwrap();
        assert_eq!(b_msgs[0].content, "SYS"); // system 提示词被覆盖
        assert!(b_msgs.last().unwrap().content.contains("Latest observation"));
        assert!(b_msgs.last().unwrap().content.contains("hello"));
    }

    #[tokio::test]
    async fn long_observation_is_truncated() {
        let backend = MockBackend::new("ok");
        let agent = LlmAgent::new(AgentConfig::default(), backend, "m");
        let big = json!({"blob": "x".repeat(5000)});
        let action = agent.act(&turn_input(Some(big), vec![])).await.unwrap();
        assert!(matches!(action, AgentAction::Reply(_)));

        let sent = agent.backend.last_messages.lock().unwrap();
        let last = &sent.last().unwrap().content;
        assert!(last.chars().count() < 1200, "observation must be truncated");
    }

    #[tokio::test]
    async fn finish_outcome_still_handled_by_engine_not_agent() {
        // LlmAgent 只产出 Reply；Finish 由编排层决定——此处仅确认动作空间未被污染
        let backend = MockBackend::new("text");
        let agent = LlmAgent::new(AgentConfig::default(), backend, "m");
        let action = agent.act(&turn_input(None, vec![])).await.unwrap();
        assert_ne!(action, AgentAction::Finish(AgentOutcome::Success));
    }
}
