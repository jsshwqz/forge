//! Reviewer Agent（AGENT-T-001）：步骤输出 + 验收结果 → 审查裁决。
//!
//! 裁决三值：`Pass`（放行）/ `Concern`（放行但记录关切）/ `Reject`（一票否决，
//! 必须阻断 Gate → EscalateHuman）。verdict 以 `EvidenceKind::Log` 固化入库。

use crate::role::ModelTier;
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_plan_llm::ChatMessage;
use std::sync::Arc;

/// 审查裁决。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ReviewVerdict {
    Pass,
    Concern,
    Reject,
}

impl ReviewVerdict {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pass" => Some(ReviewVerdict::Pass),
            "concern" => Some(ReviewVerdict::Concern),
            "reject" => Some(ReviewVerdict::Reject),
            _ => None,
        }
    }
}

/// 单条审查输入摘要。
#[derive(Clone, Debug)]
pub struct ReviewItem {
    /// 标题（如验收标准 ID 或步骤 ID）。
    pub label: String,
    /// 内容（验证结论/步骤输出，调用方负责截断）。
    pub detail: String,
}

/// 审查输入。
#[derive(Clone, Debug)]
pub struct ReviewInput {
    /// 任务目标。
    pub task_goal: String,
    /// 验收/执行摘要条目。
    pub items: Vec<ReviewItem>,
}

impl ReviewInput {
    pub fn new(task_goal: impl Into<String>) -> Self {
        Self { task_goal: task_goal.into(), items: Vec::new() }
    }

    pub fn push(mut self, label: impl Into<String>, detail: impl Into<String>) -> Self {
        self.items.push(ReviewItem { label: label.into(), detail: detail.into() });
        self
    }
}

/// 审查结论。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewOutcome {
    pub verdict: ReviewVerdict,
    pub reason: String,
}

/// 审查者抽象（便于 mock 注入）。
#[async_trait]
pub trait StepReviewer: Send + Sync {
    async fn review(&self, input: &ReviewInput) -> ForgeResult<ReviewOutcome>;
}

/// LLM 驱动的审查者（高档模型）。
pub struct LlmStepReviewer<B: forge_plan_llm::LlmPlanBackend + ?Sized> {
    pub backend: Arc<B>,
    /// 解析后的高档模型名（由 TierRouter.resolve(High) 得出）。
    pub model: String,
    pub schema_max_attempts: u32,
    /// 成本账本。
    pub ledger: Option<Arc<forge_plan_llm::UsageLedger>>,
    /// 档位标记（写成本事件用）。
    pub tier: ModelTier,
}

impl<B: forge_plan_llm::LlmPlanBackend + ?Sized> LlmStepReviewer<B> {
    fn build_messages(&self, input: &ReviewInput) -> Vec<ChatMessage> {
        let system = "You are a rigorous code-review gatekeeper for an automated delivery \
pipeline. Given the task goal and evidence items (acceptance results / step outputs), \
output a verdict. Respond with ONLY a JSON object (no prose, no code fences): \
{\"verdict\":\"Pass|Concern|Reject\",\"reason\":\"<one paragraph>\"}. \
Use \"Pass\" only when every item is satisfied with no doubt; \
use \"Concern\" when acceptable but risky; use \"Reject\" when any item fails or looks wrong."
            .to_string();

        let mut user = format!("Task goal:\n{}\n\nEvidence items:\n", input.task_goal);
        for it in &input.items {
            user.push_str(&format!("- [{}] {}\n", it.label, it.detail));
        }

        vec![ChatMessage::system(system), ChatMessage::user(user)]
    }

    fn parse_outcome(&self, raw: &str) -> ForgeResult<ReviewOutcome> {
        let json_str = forge_plan_llm::extract_json_str(raw);
        let v: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| ForgeError::InvalidState(format!("review json: {e}")))?;
        let verdict = v
            .get("verdict")
            .and_then(|x| x.as_str())
            .and_then(ReviewVerdict::parse)
            .ok_or_else(|| {
                ForgeError::InvalidState("review: missing/invalid 'verdict'".into())
            })?;
        let reason = v
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(ReviewOutcome { verdict, reason })
    }
}

#[async_trait]
impl<B: forge_plan_llm::LlmPlanBackend + ?Sized + 'static> StepReviewer for LlmStepReviewer<B> {
    async fn review(&self, input: &ReviewInput) -> ForgeResult<ReviewOutcome> {
        let mut messages = self.build_messages(input);
        let mut last_error = String::new();

        for attempt in 0..self.schema_max_attempts.max(1) {
            let (raw, usage) =
                self.backend.complete_with_usage(&self.model, &messages).await?;
            if let (Some(ledger), Some(u)) = (&self.ledger, usage) {
                ledger.record(forge_plan_llm::CostEntry {
                    model: self.model.clone(),
                    purpose: format!("review({:?})", self.tier),
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                });
            }

            match self.parse_outcome(&raw) {
                Ok(o) => return Ok(o),
                Err(e) => {
                    last_error = e.to_string();
                    if attempt + 1 < self.schema_max_attempts.max(1) {
                        messages.push(ChatMessage::assistant(raw.clone()));
                        messages.push(ChatMessage::user(format!(
                            "Output invalid:\n{last_error}\n\
Please respond again with ONLY the JSON object."
                        )));
                    }
                }
            }
        }

        Err(ForgeError::InvalidState(format!(
            "review rejected after {} attempts. Last error: {}",
            self.schema_max_attempts.max(1),
            last_error
        )))
    }
}
