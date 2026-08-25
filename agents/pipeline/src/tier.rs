//! 分层路由（AGENT-P-002）：档位 → 模型名解析。
//!
//! env 约定（契约 5.2）：
//! - `FORGE_TIER_HIGH_MODEL`：高档模型名（战略/审查）
//! - `FORGE_TIER_LOW_MODEL`：低档模型名（执行）；未配置时回落高档并告警
//!
//! 设计决策（R6）：同一供应商端点下 LlmClient 实例无差别，"tier → 实例映射"
//! 收敛为 "tier → 模型名解析"，调用点以 `resolve(tier)` 结果作为 model 参数。

use crate::role::ModelTier;
use forge_core::{ForgeError, ForgeResult};
use std::sync::atomic::{AtomicBool, Ordering};

/// 高档环境变量名。
pub const ENV_HIGH: &str = "FORGE_TIER_HIGH_MODEL";
/// 低档环境变量名。
pub const ENV_LOW: &str = "FORGE_TIER_LOW_MODEL";

/// 档位路由器。
#[derive(Clone, Debug)]
pub struct TierRouter {
    high_model: String,
    low_model: Option<String>,
}

impl TierRouter {
    /// 显式构造（测试/编程配置用）。
    pub fn from_parts(high_model: impl Into<String>, low_model: Option<String>) -> Self {
        Self { high_model: high_model.into(), low_model }
    }

    /// 从环境变量读取；HIGH 必填，LOW 可缺省（回落 High）。
    pub fn from_env() -> ForgeResult<Self> {
        let high = std::env::var(ENV_HIGH).map_err(|_| {
            ForgeError::InvalidState(format!("tier routing: {ENV_HIGH} not set"))
        })?;
        let low = std::env::var(ENV_LOW).ok().filter(|s| !s.is_empty());
        Ok(Self::from_parts(high, low))
    }

    /// 解析档位对应的模型名。Low 未配置时回落 High 并只告警一次。
    pub fn resolve(&self, tier: ModelTier) -> &str {
        match tier {
            ModelTier::High => &self.high_model,
            ModelTier::Low => match &self.low_model {
                Some(m) => m,
                None => {
                    warn_low_fallback(&self.high_model);
                    &self.high_model
                }
            },
        }
    }

    /// 低档是否可用（未配置即回落）。
    pub fn has_low(&self) -> bool {
        self.low_model.is_some()
    }

    /// 高档模型名。
    pub fn high(&self) -> &str {
        &self.high_model
    }
}

/// Low 回落告警（每进程一次，防日志刷屏）。
fn warn_low_fallback(high: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::SeqCst) {
        eprintln!(
            "[tier-router] FORGE_TIER_LOW_MODEL 未配置，Low 档调用回落 High（model={high}）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_configured_low() {
        let r = TierRouter::from_parts("m-high", Some("m-low".into()));
        assert_eq!(r.resolve(ModelTier::High), "m-high");
        assert_eq!(r.resolve(ModelTier::Low), "m-low");
        assert!(r.has_low());
    }

    #[test]
    fn falls_back_to_high_when_low_missing() {
        let r = TierRouter::from_parts("m-high", None);
        assert_eq!(r.resolve(ModelTier::Low), "m-high");
        assert!(!r.has_low());
    }

    #[test]
    fn empty_low_treated_as_missing() {
        let r = TierRouter::from_parts("m-high", Some(String::new()));
        // from_parts 不清洗空串，但 from_env 会过滤 —— 这里验证 resolve 对空串仍直通
        assert_eq!(r.resolve(ModelTier::Low), "");
    }

    #[test]
    fn from_env_requires_high() {
        // 单一 env 触碰测试（避免并行竞态），结束必清理
        std::env::remove_var(ENV_HIGH);
        std::env::remove_var(ENV_LOW);
        assert!(TierRouter::from_env().is_err());

        std::env::set_var(ENV_HIGH, "h1");
        let r = TierRouter::from_env().unwrap();
        assert_eq!(r.high(), "h1");
        assert!(!r.has_low());

        std::env::set_var(ENV_LOW, "l1");
        let r = TierRouter::from_env().unwrap();
        assert!(r.has_low());

        std::env::remove_var(ENV_HIGH);
        std::env::remove_var(ENV_LOW);
    }
}
