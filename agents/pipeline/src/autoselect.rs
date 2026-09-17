// 自动模型选择（MODEL-A-001）
//
// 背景：此前模型是"人手配一个主模型名"，所有角色（规划/审查/执行/审计）
// 共用同一个字符串，风险与角色差异无法影响模型选择。
//
// 本模块职责：在编排任务时，根据「角色 x 风险等级 x 上下文需求 x 预算」
// 自动为每个角色挑出最合适的模型，并把决策理由写入会话时间线，做到可审计。
//
// 优先级规则（与工作台/环境变量的手动选择共存）：
// 1. 显式强制指定（ModelSelector::forced）-> 最高优先级；
// 2. 自动选择 -> 本模块的确定性算法（无 LLM 参与，毫秒级）；
// 3. 放宽 -> 没有候选满足能力下限时，放宽到"可用中最优"并标记 downgraded。
//
// 确定性：给定同一组候选与同一 AutoContext，结果完全一致（同分按 id 字典序），
// 便于回归测试与审计。

use crate::role::Role;
use crate::role::ModelTier;
use crate::tier::TierRouter;
use forge_core::error::ForgeError;
use serde::{Deserialize, Serialize};

// 风险等级（对齐 AGENTS.md 的执行纪律 a 节）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    // 纯读取/查看/检查
    ReadOnly,
    // 机械/局部修改
    #[default]
    Mechanical,
    // bug 修复、重构、跨文件变更
    HighRisk,
    // 不可逆/外部影响操作
    Irreversible,
}

impl RiskLevel {
    // 中文标签（供工作台展示）
    pub fn label_zh(&self) -> &'static str {
        match self {
            RiskLevel::ReadOnly => "只读",
            RiskLevel::Mechanical => "机械改动",
            RiskLevel::HighRisk => "高风险",
            RiskLevel::Irreversible => "不可逆",
        }
    }

    // 风险对能力下限的偏移
    fn capability_delta(&self) -> f64 {
        match self {
            RiskLevel::ReadOnly => -0.20,
            RiskLevel::Mechanical => -0.10,
            RiskLevel::HighRisk => 0.05,
            RiskLevel::Irreversible => 0.15,
        }
    }
}

// 模型能力下限 = 角色基准 + 风险偏移，钳制到 [0, 1]
// 例：Architect + Irreversible = 0.90；Worker + Mechanical = 0.20
pub fn min_capability(role: Role, risk: RiskLevel) -> f64 {
    let base = match role {
        Role::Architect => 0.75,
        Role::Reviewer => 0.80,
        Role::Tester => 0.50,
        Role::Builder => 0.30,
    };
    (base + risk.capability_delta()).clamp(0.0, 1.0)
}

// 模型候选（能力元数据）
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCandidate {
    pub id: String,
    pub display: String,
    // 能力分 0.0..=1.0（越高越强）
    pub capability: f64,
    // 每百万 token 成本（0.0 = 免费/自托管）
    pub cost_per_mtok: f64,
    // 上下文窗口 token 数
    pub context_limit: usize,
    pub supports_json: bool,
    // 当前是否真的可用（已配置端点与密钥）
    pub enabled: bool,
}

impl ModelCandidate {
    pub fn new(
        id: impl Into<String>,
        capability: f64,
        cost_per_mtok: f64,
        context_limit: usize,
        supports_json: bool,
    ) -> Self {
        let id = id.into();
        Self {
            display: id.clone(),
            id,
            capability,
            cost_per_mtok,
            context_limit,
            supports_json,
            enabled: true,
        }
    }

    // 根据能力分归入既有档位（兼容 TierRouter 语义）
    pub fn tier(&self) -> ModelTier {
        if self.capability >= 0.8 {
            ModelTier::High
        } else {
            ModelTier::Low
        }
    }
}

// 内置模型能力目录（对齐 server::routes::llm::PRESETS 中的真实模型名）
// 默认全部 enabled=false：目录只用于展示与扩展，只有被 from_tier /
// set_enabled 显式启用的候选才参与自动选择，避免选中没有密钥的模型。
pub fn default_catalog() -> Vec<ModelCandidate> {
    vec![
        ModelCandidate::new("deepseek-chat", 0.86, 0.28, 65536, true),
        ModelCandidate::new("deepseek-reasoner", 0.88, 2.19, 65536, true),
        ModelCandidate::new("qwen-plus", 0.82, 0.80, 131072, true),
        ModelCandidate::new("qwen-max", 0.90, 2.40, 32768, true),
        ModelCandidate::new("qwen2.5-coder-32b-instruct", 0.72, 0.20, 32768, true),
        ModelCandidate::new("sensenova-6.8-flash-lite", 0.80, 0.25, 131072, true),
        ModelCandidate::new("gpt-4o", 0.95, 5.00, 128000, true),
        ModelCandidate::new("gpt-4o-mini", 0.78, 0.15, 128000, true),
        ModelCandidate::new("qwen2.5-coder:7b", 0.55, 0.0, 32768, true),
        ModelCandidate::new("qwen2.5-coder:14b", 0.68, 0.0, 32768, true),
        ModelCandidate::new("llama3.1:8b", 0.50, 0.0, 128000, true),
    ]
    .into_iter()
    .map(|c| ModelCandidate {
        enabled: false,
        ..c
    })
    .collect()
}

// 自动选择的输入上下文
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoContext {
    // 选择发生的角色
    pub role: Role,
    // 风险等级
    pub risk: RiskLevel,
    // 预估输入 token 需求（用于上下文窗口硬过滤）
    pub prompt_tokens: usize,
    // 剩余预算（token）；None 表示不限
    pub remaining_budget_tokens: Option<u64>,
}

impl AutoContext {
    pub fn new(role: Role, risk: RiskLevel, prompt_tokens: usize) -> Self {
        Self {
            role,
            risk,
            prompt_tokens,
            remaining_budget_tokens: None,
        }
    }

    pub fn with_budget(mut self, remaining_budget_tokens: Option<u64>) -> Self {
        self.remaining_budget_tokens = remaining_budget_tokens;
        self
    }
}

// 自动选择结果（可序列化，写入会话时间线与报告，供审计）
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSelection {
    pub model_id: String,
    pub display: String,
    pub tier: ModelTier,
    pub role: Role,
    pub risk: RiskLevel,
    // 本次要求的能力下限
    pub min_capability: f64,
    // 实际能力分
    pub capability: f64,
    // 是否发生降级（放宽能力下限 / 预算压力降级）
    pub downgraded: bool,
    // 可审计的决策理由（按产生顺序）
    pub reasons: Vec<String>,
}

// 模型选择器：持有候选清单与可选的"强制指定"
pub struct ModelSelector {
    candidates: Vec<ModelCandidate>,
    manual: Option<String>,
}

impl Clone for ModelSelector {
    fn clone(&self) -> Self {
        Self {
            candidates: self.candidates.clone(),
            manual: self.manual.clone(),
        }
    }
}

impl ModelSelector {
    // 用候选清单构造选择器（无强制指定 -> 自动选择）
    pub fn new(candidates: Vec<ModelCandidate>) -> Self {
        Self {
            candidates,
            manual: None,
        }
    }

    // 强制指定模型（手动选择优先于自动选择）
    pub fn forced(mut self, model_id: impl Into<String>) -> Self {
        self.manual = Some(model_id.into());
        self
    }

    pub fn manual(&self) -> Option<&str> {
        self.manual.as_deref()
    }

    pub fn set_manual(&mut self, model_id: Option<String>) {
        self.manual = model_id;
    }

    // 候选清单（含禁用项，供工作台展示）
    pub fn candidates(&self) -> &[ModelCandidate] {
        &self.candidates
    }

    // 当前实际可参与选择的候选
    pub fn active(&self) -> Vec<&ModelCandidate> {
        self.candidates.iter().filter(|c| c.enabled).collect()
    }

    pub fn find(&self, model_id: &str) -> Option<&ModelCandidate> {
        self.candidates.iter().find(|c| c.id == model_id)
    }

    // 启用/禁用指定候选（按 id）；返回是否找到该候选
    pub fn set_enabled(&mut self, model_id: &str, enabled: bool) -> bool {
        if let Some(c) = self.candidates.iter_mut().find(|c| c.id == model_id) {
            c.enabled = enabled;
            true
        } else {
            false
        }
    }

    // 由既有 TierRouter 桥接构造（不破坏现有环境变量配置）
    //
    // - 内置目录默认全部禁用；
    // - FORGE_TIER_HIGH_MODEL / FORGE_TIER_LOW_MODEL 对应的候选被启用；
    // - FORGE_MODEL_CANDIDATES（逗号分隔）中列出的候选也被启用；
    // - 不会把主模型当作强制指定，因此自动选择仍然生效。
    pub fn from_tier(tier: &TierRouter) -> Self {
        let mut candidates = default_catalog();
        // TierRouter 私有字段无 getter——经 resolve 拿当前模型名（High/Low 各解析一次）
        let high = tier.resolve(ModelTier::High).to_string();
        let low = tier.resolve(ModelTier::Low).to_string();
        for (id, cap, ctx) in [(high, 0.84f64, 65536usize), (low, 0.60f64, 32768usize)] {
            if id.is_empty() {
                continue;
            }
            if let Some(c) = candidates.iter_mut().find(|c| c.id == id) {
                c.enabled = true;
            } else {
                candidates.push(ModelCandidate::new(id, cap, 0.0, ctx, true));
            }
        }

        // 额外候选：FORGE_MODEL_CANDIDATES=deepseek-reasoner,qwen-max
        let extra = std::env::var("FORGE_MODEL_CANDIDATES").unwrap_or_default();
        for id in extra
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if let Some(c) = candidates.iter_mut().find(|c| c.id == id) {
                c.enabled = true;
            } else {
                candidates.push(ModelCandidate::new(id, 0.70, 0.0, 32768, true));
            }
        }

        Self::new(candidates)
    }

    // 执行选择
    //
    // 返回 Err 的情况（都是应当暴露的配置问题，不静默吞掉）：
    // - 未注册任何可用候选；
    // - 手动指定了不存在或未启用的模型 id；
    // - 没有任何候选满足硬约束（JSON 支持 / 上下文窗口）。
    pub fn select(&self, ctx: &AutoContext) -> Result<ModelSelection, ForgeError> {
        // 0) 手动指定优先
        if let Some(id) = self.manual.clone() {
            let available = self
                .candidates
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let c = self.find(&id).ok_or_else(|| {
                ForgeError::Config(format!(
                    "手动指定的模型不存在于候选清单：{id}（可用：{available}）"
                ))
            })?;
            if !c.enabled {
                return Err(ForgeError::Config(format!(
                    "手动指定的模型未启用（缺端点/密钥）：{id}"
                )));
            }
            return Ok(ModelSelection {
                model_id: c.id.clone(),
                display: c.display.clone(),
                tier: c.tier(),
                role: ctx.role,
                risk: ctx.risk,
                min_capability: min_capability(ctx.role, ctx.risk),
                capability: c.capability,
                downgraded: false,
                reasons: vec![
                    format!("手动指定（最高优先级）：{}", c.id),
                    format!("能力分 {:.2} -> 档位 {:?}", c.capability, c.tier()),
                ],
            });
        }

        let active = self.active();
        if active.is_empty() {
            return Err(ForgeError::Config(
                "未注册任何可用模型候选：请在工作台配置大模型，或通过 FORGE_MODEL_CANDIDATES 启用候选"
                    .to_string(),
            ));
        }

        // 1) 能力下限
        let min_cap = min_capability(ctx.role, ctx.risk);

        // 2) 硬过滤：（规划/审查必须支持 JSON）且上下文窗口够用
        let need = ctx.prompt_tokens;
        let need_json = matches!(ctx.role, Role::Architect | Role::Reviewer);
        let feasible: Vec<&ModelCandidate> = active
            .iter()
            .filter(|c| (!need_json || c.supports_json) && c.context_limit >= need)
            .cloned()
            .collect();

        if feasible.is_empty() {
            return Err(ForgeError::InvalidState(format!(
                "无可行模型：需要启用候选{}，上下文窗口 >= {} token（可用候选：{}）[autoselect::select role={:?} risk={:?}]",
                if need_json { "且支持 JSON 输出" } else { "" },
                need,
                active
                    .iter()
                    .map(|c| format!("{}(ctx={})", c.id, c.context_limit))
                    .collect::<Vec<_>>()
                    .join(", "),
                ctx.role,
                ctx.risk
            )));
        }

        // 3) 预算压力 -> 自动降级（允许低于能力下限，但保底 0.6 倍）
        let budget_pressure = ctx
            .remaining_budget_tokens
            .map(|b| (b as usize) < need.saturating_mul(2))
            .unwrap_or(false);

        if budget_pressure {
            let floor = (min_cap * 0.6).max(0.0);
            let relaxed_pool: Vec<&ModelCandidate> = feasible
                .iter()
                .filter(|c| c.capability >= floor)
                .cloned()
                .collect();
            let pool = if relaxed_pool.is_empty() { &feasible } else { &relaxed_pool };
            // 降级时优先最便宜；成本并列取能力更高者；再并列按 id 字典序
            let pick = pool
                .iter()
                .min_by(|a, b| {
                    a.cost_per_mtok
                        .partial_cmp(&b.cost_per_mtok)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            b.capability
                                .partial_cmp(&a.capability)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .then_with(|| a.id.cmp(&b.id))
                })
                .unwrap_or_else(|| feasible.first().unwrap());

            let mut reasons = vec![
                format!(
                    "角色 {} + 风险 {} -> 能力下限 {:.2}",
                    ctx.role.as_str(),
                    ctx.risk.label_zh(),
                    min_cap
                ),
                "预算压力（剩余预算 < 2x 本次需求）-> 启用自动降级：优先最低成本".to_string(),
            ];
            if pick.capability < min_cap {
                reasons.push(format!(
                    "能力降级：实际 {:.2} < 下限 {:.2}（保底 {:.2}）",
                    pick.capability, min_cap, floor
                ));
            }
            return Ok(ModelSelection {
                model_id: pick.id.clone(),
                display: pick.display.clone(),
                tier: pick.tier(),
                role: ctx.role,
                risk: ctx.risk,
                min_capability: min_cap,
                capability: pick.capability,
                downgraded: true,
                reasons,
            });
        }

        // 4) 正常路径：满足下限的候选中取能力最高；并列取成本最低、窗口最大、id 字典序
        let qualified: Vec<&ModelCandidate> = feasible
            .iter()
            .filter(|c| c.capability >= min_cap)
            .cloned()
            .collect();
        let relaxed = qualified.is_empty();
        let pool = if relaxed { &feasible } else { &qualified };

        let pick = pool
            .iter()
            .max_by(|a, b| {
                a.capability
                    .partial_cmp(&b.capability)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        b.cost_per_mtok
                            .partial_cmp(&a.cost_per_mtok)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| a.context_limit.cmp(&b.context_limit))
                    .then_with(|| a.id.cmp(&b.id))
            })
            .unwrap_or_else(|| feasible.first().unwrap());

        let mut reasons = vec![format!(
            "角色 {} + 风险 {} -> 能力下限 {:.2}",
            ctx.role.as_str(),
            ctx.risk.label_zh(),
            min_cap
        )];
        if relaxed {
            reasons.push(format!(
                "放宽：无候选满足能力下限（{}），取可用中最优",
                feasible
                    .iter()
                    .map(|c| format!("{}={:.2}", c.id, c.capability))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        } else {
            reasons.push(format!(
                "按能力优先选中最优：{}（能力 {:.2}，成本 ${}/1Mtok）",
                pick.id,
                pick.capability,
                pick.cost_per_mtok
            ));
        }
        reasons.push(format!(
            "硬约束通过：上下文窗口 {} >= 需求 {} token{}",
            pick.context_limit,
            need,
            if need_json { "，支持 JSON 输出" } else { "" }
        ));

        Ok(ModelSelection {
            model_id: pick.id.clone(),
            display: pick.display.clone(),
            tier: pick.tier(),
            role: ctx.role,
            risk: ctx.risk,
            min_capability: min_cap,
            capability: pick.capability,
            downgraded: relaxed,
            reasons,
        })
    }
}

// 从任务文本推断风险等级（确定性关键词检测，不依赖 LLM）
// 优先级：不可逆 > 高风险 > 只读 > 机械改动 > 默认机械改动
pub fn infer_risk(goal: &str, constraints: &[String]) -> RiskLevel {
    let text = format!("{goal}\n{}", constraints.join("\n")).to_lowercase();

    const IRREVERSIBLE: &[&str] = &[
        "删除",
        "delete",
        "drop table",
        "drop database",
        "rm -rf",
        "reset --hard",
        "force push",
        "推送",
        "push",
        "发布",
        "publish",
        "deploy",
        "上线",
        "生产环境",
        "不可逆",
        "支付",
        "付款",
        "migration",
        "数据迁移",
        "重写历史",
    ];
    const HIGH_RISK: &[&str] = &[
        "鉴权",
        "auth",
        "权限",
        "密钥",
        "secret",
        "api key",
        "安全",
        "security",
        "并发",
        "schema",
        "迁移",
        "重构",
        "refactor",
        "跨文件",
        "多文件",
        "生产配置",
        "依赖升级",
        "供应链",
        "破坏性",
        "bug",
        "修复",
        "fix",
    ];
    const READ_ONLY: &[&str] = &[
        "查看",
        "检查",
        "inspect",
        "阅读",
        "分析",
        "审查",
        "报告",
        "读一下",
        "梳理",
    ];

    if IRREVERSIBLE.iter().any(|k| text.contains(k)) {
        RiskLevel::Irreversible
    } else if HIGH_RISK.iter().any(|k| text.contains(k)) {
        RiskLevel::HighRisk
    } else if READ_ONLY.iter().any(|k| text.contains(k)) {
        RiskLevel::ReadOnly
    } else {
        RiskLevel::Mechanical
    }
}

// 粗估输入 token 数（中文按 1 字符约 0.6 token，其它按 4 字符约 1 token，外加固定开销）
pub fn estimate_prompt_tokens(goal: &str, acceptance: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in goal.chars().chain(acceptance.chars()) {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    ((cjk as f64 * 0.6) + (other as f64 / 4.0) + 200.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn make(id: &str, cap: f64, cost: f64, ctx: usize, json: bool, enabled: bool) -> ModelCandidate {
        ModelCandidate {
            id: id.to_string(),
            display: id.to_string(),
            capability: cap,
            cost_per_mtok: cost,
            context_limit: ctx,
            supports_json: json,
            enabled,
        }
    }

    // FORGE_MODEL_CANDIDATES / FORGE_TIER_* 影响全局环境变量，相关测试串行化
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn manual_override_wins_over_auto() {
        let sel = ModelSelector::new(vec![
            make("weak", 0.4, 0.01, 8192, true, true),
            make("strong", 0.95, 5.0, 128000, true, true),
        ])
        .forced("weak");
        let ctx = AutoContext::new(Role::Architect, RiskLevel::Irreversible, 100);
        let s = sel.select(&ctx).unwrap();
        assert_eq!(s.model_id, "weak");
        assert!(!s.downgraded);
        assert!(s.reasons[0].contains("手动指定"));
    }

    #[test]
    fn manual_unknown_or_disabled_id_is_config_error() {
        let ctx = AutoContext::new(Role::Architect, RiskLevel::HighRisk, 100);
        let sel = ModelSelector::new(vec![make("weak", 0.4, 0.01, 8192, true, true)]).forced("nope");
        assert!(matches!(sel.select(&ctx), Err(ForgeError::Config(_))));

        let sel2 = ModelSelector::new(vec![make("off", 0.9, 0.0, 8192, true, false)]).forced("off");
        assert!(matches!(sel2.select(&ctx), Err(ForgeError::Config(_))));
    }

    #[test]
    fn no_active_candidate_is_config_error() {
        let sel = ModelSelector::new(vec![make("x", 0.9, 0.0, 8192, true, false)]);
        let ctx = AutoContext::new(Role::Architect, RiskLevel::HighRisk, 100);
        assert!(matches!(sel.select(&ctx), Err(ForgeError::Config(_))));
    }

    #[test]
    fn architect_irreversible_prefers_highest_capability() {
        let sel = ModelSelector::new(vec![
            make("cheap", 0.70, 0.0, 8192, true, true),
            make("strong", 0.95, 5.0, 128000, true, true),
        ]);
        let s = sel
            .select(&AutoContext::new(Role::Architect, RiskLevel::Irreversible, 500))
            .unwrap();
        assert_eq!(s.model_id, "strong");
        assert!(!s.downgraded);
        assert!((s.min_capability - 0.90).abs() < 1e-9);
    }

    #[test]
    fn worker_mechanical_picks_highest_capability_of_qualified() {
        let sel = ModelSelector::new(vec![
            make("strong", 0.95, 5.0, 128000, true, true),
            make("mid", 0.70, 0.1, 32768, true, true),
        ]);
        let s = sel
            .select(&AutoContext::new(Role::Builder, RiskLevel::Mechanical, 500))
            .unwrap();
        assert_eq!(s.model_id, "strong");
        assert_eq!(s.tier, ModelTier::High);
        assert!((s.min_capability - 0.20).abs() < 1e-9);
    }

    #[test]
    fn low_capability_only_candidate_sets_low_tier() {
        let sel = ModelSelector::new(vec![make("cheap", 0.70, 0.0, 8192, true, true)]);
        let s = sel
            .select(&AutoContext::new(Role::Builder, RiskLevel::Mechanical, 500))
            .unwrap();
        assert_eq!(s.model_id, "cheap");
        assert_eq!(s.tier, ModelTier::Low);
    }

    #[test]
    fn json_required_excludes_non_json_candidates() {
        let sel = ModelSelector::new(vec![make("nojson", 0.95, 0.0, 128000, false, true)]);
        let err = sel
            .select(&AutoContext::new(Role::Reviewer, RiskLevel::HighRisk, 500))
            .unwrap_err();
        match err {
            ForgeError::InvalidState(msg) => assert!(msg.contains("无可行模型")),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn context_limit_too_small_is_validation_error() {
        let sel = ModelSelector::new(vec![make("tiny", 0.95, 0.0, 100, true, true)]);
        let ctx = AutoContext::new(Role::Reviewer, RiskLevel::HighRisk, 5000);
        match sel.select(&ctx) {
            Err(ForgeError::InvalidState(msg)) => assert!(msg.contains("无可行模型")),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn budget_pressure_downgrades_to_cheapest() {
        let sel = ModelSelector::new(vec![
            make("strong", 0.95, 5.0, 128000, true, true),
            make("cheap", 0.70, 0.0, 128000, true, true),
        ]);
        let ctx = AutoContext::new(Role::Architect, RiskLevel::HighRisk, 4000).with_budget(Some(3000));
        let s = sel.select(&ctx).unwrap();
        assert_eq!(s.model_id, "cheap");
        assert!(s.downgraded);
        assert!(s.reasons.iter().any(|r| r.contains("预算压力")));
        assert!(s.reasons.iter().any(|r| r.contains("能力降级")));
    }

    #[test]
    fn budget_pressure_above_threshold_does_not_downgrade() {
        let sel = ModelSelector::new(vec![
            make("strong", 0.95, 5.0, 128000, true, true),
            make("cheap", 0.70, 0.0, 128000, true, true),
        ]);
        let ctx = AutoContext::new(Role::Architect, RiskLevel::HighRisk, 4000).with_budget(Some(20000));
        let s = sel.select(&ctx).unwrap();
        assert_eq!(s.model_id, "strong");
        assert!(!s.downgraded);
    }

    #[test]
    fn no_candidates_below_min_relaxes_and_flags_downgraded() {
        let sel = ModelSelector::new(vec![make("weak", 0.55, 0.0, 128000, true, true)]);
        let ctx = AutoContext::new(Role::Reviewer, RiskLevel::Irreversible, 500);
        let s = sel.select(&ctx).unwrap();
        assert_eq!(s.model_id, "weak");
        assert!(s.downgraded);
        assert!(s.reasons.iter().any(|r| r.contains("放宽")));
    }

    #[test]
    fn selection_is_deterministic_with_id_tiebreak() {
        let mk = || {
            ModelSelector::new(vec![
                make("b", 0.85, 1.0, 32768, true, true),
                make("a", 0.85, 2.0, 32768, true, true),
                make("c", 0.70, 0.0, 32768, true, true),
            ])
        };
        let ctx = AutoContext::new(Role::Architect, RiskLevel::Mechanical, 500);
        assert_eq!(mk().select(&ctx).unwrap(), mk().select(&ctx).unwrap());
        // 同能力并列 -> 成本更低者
        assert_eq!(mk().select(&ctx).unwrap().model_id, "b");
    }

    #[test]
    fn min_capability_is_clamped_and_role_sensitive() {
        assert!((min_capability(Role::Architect, RiskLevel::Irreversible) - 0.90).abs() < 1e-9);
        assert!((min_capability(Role::Reviewer, RiskLevel::HighRisk) - 0.85).abs() < 1e-9);
        assert!((min_capability(Role::Builder, RiskLevel::Mechanical) - 0.20).abs() < 1e-9);
        assert!(min_capability(Role::Builder, RiskLevel::ReadOnly) >= 0.0);
        assert!(min_capability(Role::Architect, RiskLevel::Irreversible) <= 1.0);
    }

    #[test]
    fn infer_risk_maps_keywords_with_priority() {
        assert_eq!(infer_risk("把 deploy 脚本推送到生产环境", &[]), RiskLevel::Irreversible);
        assert_eq!(infer_risk("重构并删除旧模块", &[]), RiskLevel::Irreversible);
        assert_eq!(infer_risk("修复 auth 鉴权逻辑的 bug", &[]), RiskLevel::HighRisk);
        assert_eq!(infer_risk("检查这段代码并出报告", &[]), RiskLevel::ReadOnly);
        assert_eq!(infer_risk("修正文案里的 typo", &[]), RiskLevel::Mechanical);
        assert_eq!(infer_risk("写一个 fizzbuzz 脚本", &[]), RiskLevel::Mechanical);
        assert_eq!(
            infer_risk("改一下配置", &[String::from("涉及数据迁移，不可回滚")]),
            RiskLevel::Irreversible
        );
    }

    #[test]
    fn estimate_prompt_tokens_scales_with_length() {
        let small = estimate_prompt_tokens("hello", "exit code 0");
        let big = estimate_prompt_tokens(&"重构整个鉴权模块并补充单元测试".repeat(20), "");
        assert!(small < big);
        assert!(big > 200);
    }

    #[test]
    fn from_tier_enables_configured_models_and_stays_auto() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("FORGE_MODEL_CANDIDATES");
        std::env::set_var("FORGE_TIER_HIGH_MODEL", "deepseek-chat");
        std::env::set_var("FORGE_TIER_LOW_MODEL", "qwen2.5-coder:7b");

        let router = TierRouter::from_env().unwrap();
        let sel = ModelSelector::from_tier(&router);

        assert_eq!(sel.manual(), None, "from_tier 不得把主模型当成强制指定");
        let active: Vec<String> = sel.active().iter().map(|c| c.id.clone()).collect();
        assert!(active.contains(&"deepseek-chat".to_string()));
        assert!(active.contains(&"qwen2.5-coder:7b".to_string()));
        assert_eq!(active.len(), 2);

        // 自动选择仍然生效：审查角色应选能力更高的 deepseek-chat
        let s = sel
            .select(&AutoContext::new(Role::Reviewer, RiskLevel::HighRisk, 500))
            .unwrap();
        assert_eq!(s.model_id, "deepseek-chat");
    }

    #[test]
    fn from_tier_honors_extra_candidates_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("FORGE_TIER_HIGH_MODEL", "deepseek-chat");
        std::env::set_var("FORGE_TIER_LOW_MODEL", "");
        std::env::set_var("FORGE_MODEL_CANDIDATES", " qwen-max , gpt-4o-mini ");

        let router = TierRouter::from_env().unwrap();
        let sel = ModelSelector::from_tier(&router);
        let active: Vec<String> = sel.active().iter().map(|c| c.id.clone()).collect();
        assert!(active.contains(&"deepseek-chat".to_string()));
        assert!(active.contains(&"qwen-max".to_string()));
        assert!(active.contains(&"gpt-4o-mini".to_string()));
        std::env::remove_var("FORGE_MODEL_CANDIDATES");
    }

    #[test]
    fn set_enabled_toggles_candidate() {
        let mut sel = ModelSelector::new(vec![make("a", 0.9, 0.0, 8192, true, true)]);
        assert!(sel.set_enabled("a", false));
        assert!(!sel.set_enabled("missing", true));
        assert_eq!(sel.active().len(), 0);
        sel.set_enabled("a", true);
        assert_eq!(sel.active().len(), 1);
    }

    #[test]
    fn default_catalog_is_all_disabled_but_serde_roundtrips() {
        let cat = default_catalog();
        assert!(!cat.is_empty());
        assert!(cat.iter().all(|c| !c.enabled));
        let json = serde_json::to_string(&cat).unwrap();
        let back: Vec<ModelCandidate> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), cat.len());
    }
}
