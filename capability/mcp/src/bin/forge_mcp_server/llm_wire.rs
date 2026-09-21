//! MCP-004：forge_orchestrate 接入 LLM 规划器。
//!
//! 有 LLM 配置（env FORGE_LLM_BASE_URL + FORGE_LLM_API_KEY 非空）时，
//! 编排注入 `forge_plan_llm::{LlmPlanner, LlmReplanner}`（复用 server
//! B-REAL 跑通的先例），按任务 goal 自然语言生成多步计划；
//! 无配置时装配 `None` → 调用方回退 AcceptanceDrivenPlanner（离线零回归）。
//!
//! IMPROVE-10：auto_model 接 forge-pipeline/autoselect 引擎，
//! 按任务角色×风险×上下文自动选模型（偏好序降为兜底）。

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
    /// 需先探测模型（无显式 env 模型）：携带 base_url/api_key/tools + 任务上下文。
    /// IMPROVE-10：增 goal/constraints/acceptance 供 autoselect 引擎使用。
    AutoDetect {
        base_url: String,
        api_key: String,
        tools: Vec<String>,
        goal: String,
        constraints: Vec<String>,
        acceptance: String,
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
/// （list_models + autoselect 引擎按角色×风险评分，回退 OFFICIAL_MODEL_PREFS）。
/// `tools` 为执行 router 的能力白名单（防止模型发明不存在的工具）。
///
/// IMPROVE-10：wire_from_env 增 goal/constraints/acceptance 参数，
/// 传入 AutoDetect 变体供 auto_model 接引擎使用。
pub fn wire_from_env(
    tools: Vec<String>,
    goal: String,
    constraints: Vec<String>,
    acceptance: String,
) -> Option<LlmPlannerWire> {
    if !llm_configured() {
        return None;
    }
    let base_url = std::env::var("FORGE_LLM_BASE_URL").unwrap_or_default();
    let api_key = std::env::var("FORGE_LLM_API_KEY").unwrap_or_default();
    let explicit = explicit_model();
    // 未显式指定模型 → 标记由调用方在 async 上下文里 auto_model 探测
    if explicit.is_none() {
        return Some(LlmPlannerWire::AutoDetect {
            base_url,
            api_key,
            tools,
            goal,
            constraints,
            acceptance,
        });
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

/// 自动探测模型：list_models + autoselect 引擎按角色×风险评分选模型。
///
/// IMPROVE-10：接 forge-pipeline/autoselect 引擎。
/// - FORGE_MODEL_AUTOSELECT=0 时逃生阀，直接回退 pick_default_model。
/// - 引擎选出模型 → 返回 Ok(model)。
/// - 引擎无可行候选/未注册 → 回退 pick_default_model（不让 Err 冒泡打断编排）。
/// - 探测失败（list_models 网络错误）→ 返回 Err，由调用方决定回退。
pub async fn auto_model(
    base_url: &str,
    api_key: &str,
    goal: &str,
    constraints: &[String],
    acceptance: &str,
) -> forge_core::ForgeResult<String> {
    let client = LlmClient::new(base_url.to_string(), api_key.to_string());
    let ids = client.list_models().await?;

    // 逃生阀：FORGE_MODEL_AUTOSELECT=0 → 直接回退偏好序
    let autoselect_enabled = std::env::var("FORGE_MODEL_AUTOSELECT")
        .ok()
        .map(|v| v != "0")
        .unwrap_or(true); // 缺省启用

    if autoselect_enabled {
        if let Some(model) = pick_via_engine(&ids, goal, constraints, acceptance) {
            return Ok(model);
        }
        // 引擎无解 → 回退偏好序（不 Err 冒泡）
        eprintln!(
            "autoselect: engine returned None for {} candidates, falling back to OFFICIAL_MODEL_PREFS",
            ids.len()
        );
    } else {
        eprintln!("autoselect: escape hatch (FORGE_MODEL_AUTOSELECT=0), using OFFICIAL_MODEL_PREFS");
    }

    // 回退：原有偏好序选择
    forge_api::pick_default_model(&ids)
}

/// IMPROVE-10：纯函数 — 用 autoselect 引擎从可用模型 id 列表中选最优。
///
/// 逻辑冻结（build_improve10.md §3.3）：
/// 1. ModelSelector::new(default_catalog()) 构造选择器
/// 2. 对每个 provider 实有 id set_enabled(true)（只激活目录已知且可用的候选）
/// 3. AutoContext::new(Role::Architect, infer_risk(goal,constraints), estimate_prompt_tokens(goal,acceptance))
/// 4. select(&ctx).ok().map(model_id)
///
/// 返回 None = 无可行候选/目录无交集/逃生阀关闭。
fn pick_via_engine(
    ids: &[String],
    goal: &str,
    constraints: &[String],
    acceptance: &str,
) -> Option<String> {
    use forge_pipeline::autoselect::{
        default_catalog, infer_risk, estimate_prompt_tokens, AutoContext, ModelSelector,
    };
    use forge_pipeline::role::Role;

    let mut selector = ModelSelector::new(default_catalog());

    // 只激活目录已知且 provider 实有的候选
    let mut activated = 0usize;
    for id in ids {
        if selector.set_enabled(id, true) {
            activated += 1;
        }
    }
    if activated == 0 {
        eprintln!("autoselect: no catalog overlap with {} provider models", ids.len());
        return None;
    }

    let ctx = AutoContext::new(
        Role::Architect,
        infer_risk(goal, constraints),
        estimate_prompt_tokens(goal, acceptance),
    );

    match selector.select(&ctx) {
        Ok(sel) => {
            eprintln!(
                "autoselect: picked {} (tier={:?}, risk={:?}, min_cap={:.2}, cap={:.2})",
                sel.model_id, sel.tier, sel.risk, sel.min_capability, sel.capability
            );
            Some(sel.model_id)
        }
        Err(e) => {
            eprintln!("autoselect: select failed: {e}, falling back");
            None
        }
    }
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

    // ── T1: 纯装配函数（不读 env，并行安全） ──
    #[test]
    fn assembly_without_touching_env() {
        match wire_from_parts(
            "http://127.0.0.1:1/v1".into(),
            "test-key".into(),
            "test-model".into(),
            vec!["echo".into(), "write_file".into()],
        ) {
            Some(LlmPlannerWire::Ready { .. }) => {}
            _ => panic!("expected Ready, got non-Ready variant"),
        }
        // 空 key → None
        assert!(wire_from_parts("http://x/v1".into(), "".into(), "m".into(), vec![]).is_none());
    }

    // ── T2: pick_via_engine 无交集 → None ──
    // provider 返回的 id 列表与 default_catalog 完全无交集
    #[test]
    fn pick_via_engine_no_overlap() {
        let ids = vec!["unknown-model-x".into(), "another-unknown".into()];
        let result = pick_via_engine(&ids, "deploy ECS", &[], "");
        assert!(result.is_none(), "expected None for no catalog overlap");
    }

    // ── T3: pick_via_engine 有交集 → 选出目录内模型 ──
    // 传入 deepseek-chat + sensenova-6.8-flash-lite → 引擎应返回其中之一
    #[test]
    fn pick_via_engine_with_overlap() {
        let ids = vec![
            "deepseek-chat".into(),
            "sensenova-6.8-flash-lite".into(),
            "unknown-junk".into(),
        ];
        let result = pick_via_engine(&ids, "create VPC", &[], "");
        assert!(result.is_some(), "expected Some for overlapping models");
        let model = result.unwrap();
        assert!(
            model == "deepseek-chat" || model == "sensenova-6.8-flash-lite",
            "picked model should be one of the activated candidates, got: {model}"
        );
    }

    // ── T4: pick_via_engine 高风险 goal + 低能力候选 → 降级选择 ──
    // 引擎在无候选达标时放宽到"可用中最优"（downgraded=true），
    // 不返回 None — 这是有意设计：避免编排因选不到模型而中断。
    // 我们验证：低能力候选在高风险下仍被选出（降级），但不会 Err。
    #[test]
    fn pick_via_engine_high_risk_downgrades() {
        let ids = vec!["llama3.1:8b".into()]; // capability=0.50
        // 高风险 goal → min_capability = 0.75 + 0.15 = 0.90 > 0.50
        // 引擎应放宽 → 仍然返回 Some("llama3.1:8b")（降级选择）
        let result = pick_via_engine(&ids, "delete production database", &[], "");
        assert!(
            result.is_some(),
            "expected Some even under high risk: engine downgrades rather than fails"
        );
        assert_eq!(
            result.unwrap(),
            "llama3.1:8b",
            "only candidate should be picked via downgrade"
        );
    }

    // ── T5: pick_via_engine 空列表 → None ──
    #[test]
    fn pick_via_engine_empty_ids() {
        let result = pick_via_engine(&[], "do something", &[], "");
        assert!(result.is_none(), "expected None for empty id list");
    }
}
