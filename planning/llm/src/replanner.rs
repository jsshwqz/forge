//! Replanner（PLAN-R-001）：原计划+失败记录 → 修订计划。

use crate::llm_planner::LlmPlanBackend;
use crate::validator::extract_json_str;
use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_recovery::classify::FailureRecord;
use async_trait::async_trait;
use forge_planner::{Plan, PlanStatus};
use std::sync::Arc;

/// 重规划 trait。
#[async_trait]
pub trait Replanner: Send + Sync {
    /// 输入原计划与失败记录，输出修订计划。
    async fn replan(&self, plan: &Plan, failures: &[FailureRecord]) -> ForgeResult<Plan>;
}

/// LLM 驱动的重规划器。
pub struct LlmReplanner<B: LlmPlanBackend> {
    pub backend: Arc<B>,
    pub model: String,
    pub schema_max_attempts: u32,
}

#[async_trait]
impl<B: LlmPlanBackend + 'static> Replanner for LlmReplanner<B> {
    async fn replan(&self, plan: &Plan, failures: &[FailureRecord]) -> ForgeResult<Plan> {
        let system = "You are a plan revision assistant. Given an original plan and failure records, produce a REVISED plan. Respond with ONLY JSON in the same schema as the original plan.".to_string();

        let failures_text = failures
            .iter()
            .map(|f| format!("step execution failed: {:?}", f.category))
            .collect::<Vec<_>>()
            .join("\n");

        let steps_json = serde_json::to_string_pretty(&plan.steps).unwrap_or_default();

        let user = format!(
            "Original plan:\n{}\n\nFailures:\n{}\n\nProduce a revised plan. New Plan.id must be regenerated. Completed steps should be preserved. Do not retry non-retriable failures.",
            steps_json,
            failures_text,
        );

        let mut messages = vec![
            forge_api::ChatMessage::system(system),
            forge_api::ChatMessage::user(user),
        ];

        let mut last_error = String::new();

        for attempt in 0..self.schema_max_attempts {
            let raw = self.backend.complete(&self.model, &messages).await?;
            let json_str = extract_json_str(&raw);
            let parsed: serde_json::Value = serde_json::from_str(&json_str)
                .unwrap_or_else(|e| serde_json::json!({"_err": e.to_string()}));

            // 校验（复用 validator）
            let new_plan = match validate_revised_plan(&parsed, &plan.task_id) {
                Ok(p) => p,
                Err(e) => {
                    last_error = e.to_string();
                    if attempt < self.schema_max_attempts {
                        messages.push(forge_api::ChatMessage::assistant(raw.clone()));
                        messages.push(forge_api::ChatMessage::user(format!(
                            "Validation failed:\n{}\nPlease fix.", last_error
                        )));
                        continue;
                    }
                    return Err(e);
                }
            };

            return Ok(new_plan);
        }

        Err(ForgeError::InvalidState(format!(
            "replan rejected after {} attempts. Last error: {}",
            self.schema_max_attempts, last_error
        )))
    }
}

fn validate_revised_plan(raw: &serde_json::Value, task_id: &TaskId) -> ForgeResult<Plan> {
    use crate::validator;
    let mut plan = validator::validate_plan(raw, task_id)?;
    plan.status = PlanStatus::Ready;
    Ok(plan)
}
