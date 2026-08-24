//! LLM 驱动的规划器（PLAN-L-002）。
//!
//! - [`LlmPlanBackend`]：规划后端抽象（生产用 [`forge_api::LlmClient`]，测试注入 mock）
//! - [`LlmPlanner`]：prompt → LLM → schema 校验 → 失败回喂的修复循环
//!
//! 设计要点：
//! - 后端抽象与重规划器共享（[`crate::replanner::LlmReplanner`] 复用同一 trait）
//! - schema 修复循环复用 [`crate::validator::validate_plan`]，错误文案直接回喂给模型

use crate::validator::{extract_json_str, validate_plan};
use async_trait::async_trait;
use forge_api::ChatMessage;
use forge_core::{ForgeError, ForgeResult};
use forge_planner::{Plan, Planner};
use forge_task::Task;
use std::sync::Arc;

/// 规划/重规划共用的 LLM 后端抽象。
#[async_trait]
pub trait LlmPlanBackend: Send + Sync {
    /// 给定模型与消息序列，返回助手原始文本。
    async fn complete(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String>;
}

/// 生产后端：复用 OpenAI 兼容客户端（chat_raw + 内容提取 + 429 退避）。
#[async_trait]
impl LlmPlanBackend for forge_api::LlmClient {
    async fn complete(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String> {
        let raw = self.chat_raw(model, messages).await?;
        Self::extract_content(&raw)
    }
}

/// LLM 驱动的规划器：实现 [`Planner`] trait（PLAN-L-002）。
///
/// 循环：`complete → 提取 JSON → validate_plan`；
/// 校验失败时把错误文案回喂给模型，最多尝试 `schema_max_attempts` 次。
pub struct LlmPlanner<B: LlmPlanBackend> {
    pub backend: Arc<B>,
    pub model: String,
    pub schema_max_attempts: u32,
    /// 可用能力白名单：非空时写入提示词强约束（防止模型发明不存在的工具）。
    pub tools: Vec<String>,
}

impl<B: LlmPlanBackend> LlmPlanner<B> {
    pub fn new(backend: Arc<B>, model: impl Into<String>) -> Self {
        Self { backend, model: model.into(), schema_max_attempts: 3, tools: Vec::new() }
    }

    fn build_messages(&self, task: &Task) -> Vec<ChatMessage> {
        let tool_rule = if self.tools.is_empty() {
            "If unsure which capability to use, use \"echo\".".to_string()
        } else {
            format!(
                "Available capabilities: {:?}. Every call step MUST use exactly one of these; \
                 do NOT invent capability names.",
                self.tools
            )
        };
        let system = format!(
            "You are a planning assistant. Produce an execution plan for the task. \
Respond with ONLY a JSON object (no prose, no code fences) matching this schema: \
{{\"steps\":[{{\"id\":\"s1\",\"title\":\"...\",\"depends_on\":[],\
\"action\":{{\"type\":\"call\",\"capability\":\"echo\",\"input\":{{}}}}}}]}}. \
Action type must be \"call\" or \"approval\". \
Step ids must be unique; depends_on must reference existing step ids only. \
{tool_rule}"
        );

        let mut user = format!("Task goal:\n{}\n", task.goal);
        if !task.constraints.is_empty() {
            user.push_str("\nConstraints:\n");
            for c in &task.constraints {
                user.push_str(&format!("- {c}\n"));
            }
        }
        if !task.acceptance.is_empty() {
            user.push_str("\nAcceptance criteria:\n");
            for a in &task.acceptance {
                user.push_str(&format!("- {}: {}\n", a.id, a.description));
            }
        }

        vec![ChatMessage::system(system), ChatMessage::user(user)]
    }
}

#[async_trait]
impl<B: LlmPlanBackend + 'static> Planner for LlmPlanner<B> {
    async fn plan(&self, task: &Task) -> ForgeResult<Plan> {
        let mut messages = self.build_messages(task);
        let mut last_error = String::new();

        for attempt in 0..self.schema_max_attempts {
            let raw = self.backend.complete(&self.model, &messages).await?;
            let json_str = extract_json_str(&raw);
            let parsed: serde_json::Value = serde_json::from_str(&json_str)
                .unwrap_or_else(|e| serde_json::json!({"_parse_error": e.to_string()}));

            match validate_plan(&parsed, &task.id) {
                Ok(plan) => return Ok(plan),
                Err(e) => {
                    last_error = e.to_string();
                    if attempt + 1 < self.schema_max_attempts {
                        messages.push(ChatMessage::assistant(raw.clone()));
                        messages.push(ChatMessage::user(format!(
                            "Validation failed:\n{last_error}\n\
Please output a corrected plan (ONLY the JSON object)."
                        )));
                    }
                }
            }
        }

        Err(ForgeError::InvalidState(format!(
            "plan rejected after {} attempts. Last error: {}",
            self.schema_max_attempts, last_error
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::TaskId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 顺序回放预设响应的离线 mock。
    struct MockBackend {
        responses: Vec<String>,
        calls: AtomicUsize,
    }

    impl MockBackend {
        fn new(responses: Vec<&str>) -> Self {
            Self { responses: responses.into_iter().map(String::from).collect(), calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait]
    impl LlmPlanBackend for MockBackend {
        async fn complete(&self, _model: &str, _messages: &[ChatMessage]) -> ForgeResult<String> {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses.get(i).cloned().ok_or_else(|| {
                ForgeError::InvalidState("mock: no more canned responses".into())
            })
        }
    }

    fn demo_task() -> Task {
        Task::new(TaskId::new_task_id(), "write a hello file".into(), vec![], vec![])
    }

    const GOOD_PLAN: &str = r#"{"steps":[{"id":"s1","title":"t","depends_on":[]}]}"#;

    #[tokio::test]
    async fn valid_first_shot() {
        let planner = LlmPlanner::new(Arc::new(MockBackend::new(vec![GOOD_PLAN])), "mock-model");
        let plan = planner.plan(&demo_task()).await.unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].id, "s1");
    }

    #[tokio::test]
    async fn repair_loop_accepts_after_wrapped_output() {
        // 第一次输出带 markdown 围栏（可提取但缺 title 之外的合法字段仍可通过），
        // 用"空对象"制造校验失败，第二次给合法计划，验证错误被回喂且循环恢复。
        let planner = LlmPlanner::new(
            Arc::new(MockBackend::new(vec!["oops not json", GOOD_PLAN])),
            "mock-model",
        );
        let plan = planner.plan(&demo_task()).await.unwrap();
        assert_eq!(plan.steps[0].id, "s1");
    }

    #[tokio::test]
    async fn exhausted_attempts_reports_last_error() {
        let planner = LlmPlanner::new(
            Arc::new(MockBackend::new(vec![
                r#"{"steps":[{"id":"a","depends_on":["ghost"]}]}"#,
                r#"{"steps":[{"id":"a","depends_on":["ghost"]}]}"#,
                r#"{"steps":[{"id":"a","depends_on":["ghost"]}]}"#,
            ])),
            "mock-model",
        );
        let err = planner.plan(&demo_task()).await.unwrap_err();
        assert!(err.to_string().contains("after 3 attempts"));
        assert!(err.to_string().contains("ghost"));
    }

    #[tokio::test]
    async fn prompt_contains_goal_constraints_and_acceptance() {
        use forge_task::{AcceptanceCriterion, CheckSpec};
        let task = Task::new(
            TaskId::new_task_id(),
            "ship it".into(),
            vec!["no network".into()],
            vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "file exists".into(),
                check: CheckSpec::FileExists("out.txt".into()),
            }],
        );
        let planner: LlmPlanner<MockBackend> =
            LlmPlanner::new(Arc::new(MockBackend::new(vec![])), "mock-model");
        let msgs = planner.build_messages(&task);
        let user = &msgs[1].content;
        assert!(user.contains("ship it"));
        assert!(user.contains("no network"));
        assert!(user.contains("AC-1"));
        assert!(msgs[0].content.contains("ONLY a JSON object"));

        // 工具白名单非空时必须出现在系统提示词中
        let mut constrained: LlmPlanner<MockBackend> =
            LlmPlanner::new(Arc::new(MockBackend::new(vec![])), "mock-model");
        constrained.tools = vec!["echo".into()];
        let sys = constrained.build_messages(&task)[0].content.clone();
        assert!(sys.contains("\"echo\""));
    }
}
