//! 回合引擎（Turn Engine）。
//!
//! 驱动 Agent 的回合循环：输入→决策→分发→观察→下一回合。
//! 含超时、回数上限与循环检测三道护栏。

use crate::model::{Agent, AgentAction, AgentOutcome, TurnInput};
use async_trait::async_trait;
use forge_core::{ForgeResult, SessionId};
use serde::{Deserialize, Serialize};
use tracing::info;

/// 动作分发器 trait。
#[async_trait]
pub trait ActionDispatcher: Send + Sync {
    /// 分发动作并返回观察结果。
    async fn dispatch(&self, action: &AgentAction) -> ForgeResult<serde_json::Value>;
}

/// 终止原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminateReason {
    /// Agent 主动完成。
    Finished,
    /// 超过最大回合数。
    MaxTurns,
    /// Agent 主动中止。
    Aborted,
    /// 检测到循环。
    LoopDetected,
}

/// 回合报告。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnReport {
    /// 使用的回合数。
    pub turns_used: u32,
    /// 最终结果。
    pub outcome: AgentOutcome,
    /// 终止原因。
    pub terminated: TerminateReason,
    /// 所有动作记录。
    pub actions: Vec<AgentAction>,
}

/// 回合引擎。
pub struct TurnEngine<A: Agent> {
    agent: A,
    max_turns: u32,
}

impl<A: Agent> TurnEngine<A> {
    /// 创建回合引擎。
    pub fn new(agent: A, max_turns: u32) -> Self {
        Self { agent, max_turns }
    }

    /// 运行回合循环。
    pub async fn run(
        &self,
        session_id: &SessionId,
        dispatcher: &dyn ActionDispatcher,
    ) -> ForgeResult<TurnReport> {
        let mut history: Vec<serde_json::Value> = Vec::new();
        let mut actions: Vec<AgentAction> = Vec::new();
        let mut last_three: Vec<AgentAction> = Vec::new();
        let mut outcome = AgentOutcome::Failed;
        let mut reason = TerminateReason::MaxTurns;

        for turn in 1..=self.max_turns {
            let input = TurnInput {
                session_id: session_id.clone(),
                turn,
                history: history.clone(),
                observation: if turn > 1 {
                    history.last().cloned()
                } else {
                    None
                },
            };

            let action = self.agent.act(&input).await?;
            actions.push(action.clone());

            match &action {
                AgentAction::Finish(o) => {
                    outcome = *o;
                    reason = TerminateReason::Finished;
                    break;
                }
                AgentAction::Abort(msg) => {
                    outcome = AgentOutcome::Failed;
                    reason = TerminateReason::Aborted;
                    let _ = msg;
                    break;
                }
                AgentAction::Reply(_) => {
                    // Reply 不产生 observation，直接进入下一回合
                    last_three.push(action.clone());
                    if last_three.len() > 3 {
                        last_three.remove(0);
                    }
                    // 检查循环：连续 3 次相同动作
                    if last_three.len() == 3
                        && last_three[0] == last_three[1]
                        && last_three[1] == last_three[2]
                    {
                        outcome = AgentOutcome::Failed;
                        reason = TerminateReason::LoopDetected;
                        break;
                    }
                }
                AgentAction::CallTool { .. } => {
                    // 循环检测
                    last_three.push(action.clone());
                    if last_three.len() > 3 {
                        last_three.remove(0);
                    }
                    if last_three.len() == 3
                        && last_three[0] == last_three[1]
                        && last_three[1] == last_three[2]
                    {
                        outcome = AgentOutcome::Failed;
                        reason = TerminateReason::LoopDetected;
                        break;
                    }

                    // 分发并获取 observation
                    let observation = dispatcher.dispatch(&action).await?;
                    history.push(observation);
                }
            }
        }

        info!(
            session_id = %session_id,
            turns_used = actions.len() as u32,
            ?outcome,
            ?reason,
            "turn engine finished"
        );

        Ok(TurnReport {
            turns_used: actions.len() as u32,
            outcome,
            terminated: reason,
            actions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentAction, AgentConfig, AgentOutcome};
    use forge_core::{ForgeError, SessionId};
    use tokio::sync::Mutex;
    use std::collections::VecDeque;

    /// 测试夹具：按脚本返回动作。
    struct ScriptedAgent {
        config: AgentConfig,
        script: Mutex<VecDeque<AgentAction>>,
    }

    impl ScriptedAgent {
        fn new(actions: Vec<AgentAction>) -> Self {
            Self {
                config: AgentConfig::default(),
                script: Mutex::new(actions.into()),
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
                .ok_or_else(|| ForgeError::InvalidState("script exhausted".into()))
        }
    }

    /// 测试夹具：原样返回输入的分发器。
    struct EchoDispatcher;

    #[async_trait]
    impl ActionDispatcher for EchoDispatcher {
        async fn dispatch(&self, action: &AgentAction) -> ForgeResult<serde_json::Value> {
            Ok(serde_json::json!({"action": format!("{:?}", action)}))
        }
    }

    #[tokio::test]
    async fn test_normal_finish() {
        let agent = ScriptedAgent::new(vec![
            AgentAction::CallTool {
                tool: "echo".into(),
                input: serde_json::json!({}),
            },
            AgentAction::Finish(AgentOutcome::Success),
        ]);
        let engine = TurnEngine::new(agent, 50);
        let session_id = SessionId::new_session_id();

        let report = engine.run(&session_id, &EchoDispatcher).await.unwrap();
        assert_eq!(report.outcome, AgentOutcome::Success);
        assert_eq!(report.terminated, TerminateReason::Finished);
        assert_eq!(report.turns_used, 2);
        assert_eq!(report.actions.len(), 2);
    }

    #[tokio::test]
    async fn test_loop_detected() {
        let agent = ScriptedAgent::new(vec![
            AgentAction::CallTool {
                tool: "echo".into(),
                input: serde_json::json!({}),
            },
            AgentAction::CallTool {
                tool: "echo".into(),
                input: serde_json::json!({}),
            },
            AgentAction::CallTool {
                tool: "echo".into(),
                input: serde_json::json!({}),
            },
            AgentAction::Finish(AgentOutcome::Success),
        ]);
        let engine = TurnEngine::new(agent, 50);
        let session_id = SessionId::new_session_id();

        let report = engine.run(&session_id, &EchoDispatcher).await.unwrap();
        assert_eq!(report.terminated, TerminateReason::LoopDetected);
        assert_eq!(report.outcome, AgentOutcome::Failed);
        assert_eq!(report.turns_used, 3);
    }

    #[tokio::test]
    async fn test_max_turns() {
        // 无限 Reply
        let actions: Vec<AgentAction> = (0..100)
            .map(|i| AgentAction::Reply(format!("msg {}", i)))
            .collect();
        let agent = ScriptedAgent::new(actions);
        let engine = TurnEngine::new(agent, 5);
        let session_id = SessionId::new_session_id();

        let report = engine.run(&session_id, &EchoDispatcher).await.unwrap();
        assert_eq!(report.terminated, TerminateReason::MaxTurns);
        assert_eq!(report.outcome, AgentOutcome::Failed);
    }

    #[tokio::test]
    async fn test_abort() {
        let agent = ScriptedAgent::new(vec![
            AgentAction::Abort("something went wrong".into()),
        ]);
        let engine = TurnEngine::new(agent, 50);
        let session_id = SessionId::new_session_id();

        let report = engine.run(&session_id, &EchoDispatcher).await.unwrap();
        assert_eq!(report.terminated, TerminateReason::Aborted);
        assert_eq!(report.outcome, AgentOutcome::Failed);
        assert_eq!(report.turns_used, 1);
    }
}
