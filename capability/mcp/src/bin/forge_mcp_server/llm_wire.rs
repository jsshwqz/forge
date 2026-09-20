//! MCP-004：forge_orchestrate 接入 LLM 规划器。
//!
//! 有 LLM 配置（env FORGE_LLM_BASE_URL + FORGE_LLM_API_KEY 非空）时，
//! 编排注入 `forge_plan_llm::{LlmPlanner, LlmReplanner}`（复用 server
//! B-REAL 跑通的先例），按任务 goal 自然语言生成多步计划；
//! 无配置时装配 `None` → 调用方回退 AcceptanceDrivenPlanner（离线零回归）。

use forge_api::{LlmBackend, LlmClient};
use forge_plan_llm::llm_planner::LlmPlanBackend;
use forge_plan_llm::{LlmPlanner, LlmReplanner};
use std::sync::Arc;

/// LLM 装配结果：planner + replanner（None = 未配置，回退确定性规划）。
pub enum LlmPlannerWire {
    /// 已就绪：planner + replanner 直接可注入。
    Ready {
        planner: Arc<dyn forge_planner::Planner>,
        replanner: Arc<dyn forge_plan_llm::Replanner>,
    },
    /// 需先探测模型（无显式 env 模型）：携带 base_url/api_key/tools。
    AutoDetect {
        base_url: String,
        api_key: String,
        tools: Vec<String>,
    },
}

/// 是否已配置 LLM（base_url + api_key 均非空）。
pub fn llm_configured() -> bool {
    let base = std::env::var("FORGE_LLM_BASE_URL").unwrap_or_default();
    let key = std::env::var("FORGE_LLM_API_KEY").unwrap_or_default();
    !base.trim().is_empty() && !key.trim().is_empty()
}

/// 从 env 装配 LLM 规划器/重规划器。
///
/// 模型取值顺序：FORGE_TIER_HIGH_MODEL → FORGE_LLM_MODEL → 自动探测
/// （list_models + OFFICIAL_MODEL_PREFS 偏好，6.8 优先）。
/// `tools` 为执行 router 的能力白名单（防止模型发明不存在的工具）。
pub fn wire_from_env(tools: Vec<String>) -> Option<LlmPlannerWire> {
    if !llm_configured() {
        return None;
    }
    let base_url = std::env::var("FORGE_LLM_BASE_URL").unwrap_or_default();
    let api_key = std::env::var("FORGE_LLM_API_KEY").unwrap_or_default();
    let explicit = explicit_model();
    // 未显式指定模型 → 标记由调用方在 async 上下文里 auto_model 探测
    if explicit.is_none() {
        return Some(LlmPlannerWire::AutoDetect { base_url, api_key, tools });
    }
    wire_from_parts(base_url, api_key, explicit.unwrap(), tools)
}

/// env 显式指定的模型（FORGE_TIER_HIGH_MODEL → FORGE_LLM_MODEL）；无则 None。
pub fn explicit_model() -> Option<String> {
    std::env::var("FORGE_TIER_HIGH_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("FORGE_LLM_MODEL").ok().filter(|s| !s.trim().is_empty()))
}

/// 自动探测模型：GET /models + 官方偏好序选择（6.8 → 6.7 → glm → chat）。
/// 探测失败返回 Err，由调用方决定回退（不再静默指定）。
pub async fn auto_model(base_url: &str, api_key: &str) -> forge_core::ForgeResult<String> {
    let client = LlmClient::new(base_url.to_string(), api_key.to_string());
    let ids = client.list_models().await?;
    forge_api::pick_default_model(&ids)
}

/// 纯装配函数（可测，不读 env；由 [`wire_from_env`] 读取配置后调用）。
pub fn wire_from_parts(
    base_url: String,
    api_key: String,
    model: String,
    tools: Vec<String>,
) -> Option<LlmPlannerWire> {
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return None;
    }
    let backend: Arc<dyn LlmPlanBackend> = Arc::new(LlmClient::new(base_url, api_key));
    let planner: Arc<dyn forge_planner::Planner> = Arc::new(LlmPlanner {
        backend: backend.clone(),
        model: model.clone(),
        schema_max_attempts: 3,
        tools: tools.clone(),
        ledger: None,
        meter: None,
        brief_mode: false,
        context: None,
    });
    let replanner: Arc<dyn forge_plan_llm::Replanner> = Arc::new(LlmReplanner {
        backend,
        model,
        schema_max_attempts: 3,
        tools,
        ledger: None,
        meter: None,
    });
    Some(LlmPlannerWire::Ready { planner, replanner })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_without_touching_env() {
        // 只测纯装配函数，不 set/remove 全局 env（避免并行测试互踩）。
        match wire_from_parts(
            "http://127.0.0.1:1/v1".into(),
            "test-key".into(),
            "test-model".into(),
            vec!["echo".into(), "write_file".into()],
        ) {
            Some(LlmPlannerWire::Ready { .. }) => {}
            _ => panic!("expected Ready, got non-Ready variant"),
        }

        // 空 key → None（未配置语义）
        assert!(wire_from_parts("http://x/v1".into(), "".into(), "m".into(), vec![]).is_none());
    }
}