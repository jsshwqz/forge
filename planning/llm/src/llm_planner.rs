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

    /// 同 [`complete`](Self::complete)，并尽力返回 token 用量（供应商未回 usage 时为 None）。
    /// 默认实现退化为 [`complete`](Self::complete)（无用量）。
    async fn complete_with_usage(
        &self,
        model: &str,
        messages: &[ChatMessage],
    ) -> ForgeResult<(String, Option<crate::usage::TokenUsage>)> {
        let text = self.complete(model, messages).await?;
        Ok((text, None))
    }
}

/// 生产后端：复用 OpenAI 兼容客户端（chat_raw + 内容提取 + 429 退避）。
#[async_trait]
impl LlmPlanBackend for forge_api::LlmClient {
    async fn complete(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String> {
        let raw = self.chat_raw(model, messages).await?;
        Self::extract_content(&raw)
    }

    async fn complete_with_usage(
        &self,
        model: &str,
        messages: &[ChatMessage],
    ) -> ForgeResult<(String, Option<crate::usage::TokenUsage>)> {
        use crate::usage::TokenUsage;
        let raw = self.chat_raw(model, messages).await?;
        let text = Self::extract_content(&raw)?;
        let usage = TokenUsage {
            prompt_tokens: raw
                .pointer("/usage/prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            completion_tokens: raw
                .pointer("/usage/completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        };
        // 供应商未回 usage 时两值均为 0 —— 视作不可得，交 None
        let has_usage = raw.pointer("/usage").is_some();
        Ok((text, if has_usage { Some(usage) } else { None }))
    }
}

/// LLM 驱动的规划器：实现 [`Planner`] trait（PLAN-L-002）。
///
/// 循环：`complete → 提取 JSON → validate_plan`；
/// 校验失败时把错误文案回喂给模型，最多尝试 `schema_max_attempts` 次。
pub struct LlmPlanner<B: LlmPlanBackend + ?Sized> {
    pub backend: Arc<B>,
    pub model: String,
    pub schema_max_attempts: u32,
    /// 可用能力白名单：非空时写入提示词强约束（防止模型发明不存在的工具）。
    pub tools: Vec<String>,
    /// 可选成本账本：每次成功调用后记账（G-V3.2 成本记录）。
    pub ledger: Option<Arc<crate::usage::UsageLedger>>,
    /// BILL-003 R6-026 先例：token 计量钩子（usage_events 流水面，默认 None）。
    pub meter: Option<Arc<dyn crate::usage::LlmMeter>>,
    /// 简述模式（服务端"写软件"路径）：write_file 步骤只要求 `brief`
    /// （文件意图），不内嵌完整 content——由调用方二次代码生成填充，
    /// 规避小上限模型在长 JSON 转义上的截断问题。
    pub brief_mode: bool,
    /// V8 CTX-001：工作区上下文注入（已组装的"清单+小文件内容"文本，≤32KB），
    /// 追加到 user 消息尾部，让模型"看得见"既有工作区。
    pub context: Option<String>,
}

impl<B: LlmPlanBackend + ?Sized> LlmPlanner<B> {
    pub fn new(backend: Arc<B>, model: impl Into<String>) -> Self {
        Self {
            backend,
            model: model.into(),
            schema_max_attempts: 3,
            tools: Vec::new(),
            ledger: None,
            meter: None,
            brief_mode: false,
            context: None,
        }
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
        let write_rule = if self.tools.iter().any(|t| t == "write_file") {
            if self.brief_mode {
                "\nFor write_file steps use input {\"path\":\"<relative path>\",\
                 \"brief\":\"<one-sentence description of what the file must contain>\"}. \
                 Do NOT include the file content itself."
                    .to_string()
            } else {
                // V7 ORCH-101 真实轨修复：非 brief 模式必须冻结 write_file input
                // 形状（path+content），否则模型自由发挥导致 missing 'path'
                "\nFor write_file steps use input {\"path\":\"<relative path with extension>\",\
                 \"content\":\"<complete final file content, escaped for JSON>\"}. \
                 The content MUST be the complete file content, not a description."
                    .to_string()
            }
        } else {
            String::new()
        };
        let system = format!(
            "You are a planning assistant. Produce an execution plan for the task. \
Respond with ONLY a JSON object (no prose, no code fences) matching this schema: \
{{\"steps\":[{{\"id\":\"s1\",\"title\":\"...\",\"depends_on\":[],\
\"action\":{{\"type\":\"call\",\"capability\":\"echo\",\"input\":{{}}}}}}]}}. \
Action type must be \"call\" or \"approval\". \
Step ids must be unique; depends_on must reference existing step ids only. \
{tool_rule}{write_rule}"
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
        // V8 CTX-001：工作区上下文注入（冻结格式，追加 user 尾部）。
        if let Some(ctx) = &self.context {
            user.push_str("\n=== Workspace context ===\n");
            user.push_str(ctx);
        }

        vec![ChatMessage::system(system), ChatMessage::user(user)]
    }
}

#[async_trait]
impl<B: LlmPlanBackend + ?Sized + 'static> Planner for LlmPlanner<B> {
    async fn plan(&self, task: &Task) -> ForgeResult<Plan> {
        let mut messages = self.build_messages(task);
        let mut last_error = String::new();

        for attempt in 0..self.schema_max_attempts {
            let (raw, usage) = self.backend.complete_with_usage(&self.model, &messages).await?;
            if let (Some(ledger), Some(u)) = (&self.ledger, usage) {
                ledger.record(crate::usage::CostEntry {
                    model: self.model.clone(),
                    purpose: "plan".into(),
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                });
            }
            // BILL-003：usage_events 流水面（与 ledger 并行，R4 不合并）
            if let (Some(m), Some(u)) = (&self.meter, usage) {
                m.on_usage(&self.model, "plan", u.prompt_tokens, u.completion_tokens);
            }
            // 观测：模型原始输出截断打印（排障用）
            eprintln!(
                "llm_planner[{}] raw_len={} head={:?}",
                attempt + 1,
                raw.len(),
                raw.chars().take(120).collect::<String>()
            );
            let mut parsed: serde_json::Value = {
                let s = extract_json_str(&raw);
                serde_json::from_str(&s).unwrap_or_else(|e| {
                    serde_json::json!({"_parse_error": format!("{e}: {}", s.chars().take(120).collect::<String>())})
                })
            };
            // 容错：部分模型直接返回步骤数组而非对象
            if parsed.is_array() {
                parsed = serde_json::json!({ "steps": parsed });
            }

            if let Some(err) = parsed.get("_parse_error").and_then(|v| v.as_str()) {
                last_error = format!("JSON syntax error: {err}");
                eprintln!("llm_planner[{}] parse error: {err}", attempt + 1);
                if attempt + 1 < self.schema_max_attempts {
                    messages.push(ChatMessage::assistant(raw.clone()));
                    messages.push(ChatMessage::user(format!(
                        "Validation failed:\n{last_error}\n\
Please output a valid, well-escaped JSON object matching the schema. Ensure all quotes inside strings are escaped like \\\" and no unescaped control characters."
                    )));
                }
                continue;
            }

            match validate_plan(&parsed, &task.id) {
                Ok(plan) => return Ok(plan),
                Err(e) => {
                    eprintln!(
                        "llm_planner[{}] FAILED: {e} | parsed_keys={:?} | tail={:?}",
                        attempt + 1,
                        parsed.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
                        raw.chars().rev().take(150).collect::<String>()
                    );
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

        // V7 ORCH-101 冻结：write_file 工具且非 brief 模式 → 系统提示词必须
        // 冻结 input 形状（path + content），防模型自由发挥
        let mut wf: LlmPlanner<MockBackend> =
            LlmPlanner::new(Arc::new(MockBackend::new(vec![])), "mock-model");
        wf.tools = vec!["echo".into(), "write_file".into()];
        let sys2 = wf.build_messages(&task)[0].content.clone();
        assert!(sys2.contains("\"path\"") && sys2.contains("\"content\""),
            "write_file 全内容规则必须在提示词中: {sys2}");
    }
}
