//! forge-plan-llm：LLM 驱动规划 + 重规划（V3.1）。
//!
//! - [`validator::validate_plan`]：校验 LLM 原始 JSON → 合法 Plan（纯函数）
//! - [`LlmPlanner`]：实现 Planner trait，prompt→LLM→校验→repair 循环
//! - [`Replanner`]：原计划+失败记录 → 修订计划

pub mod llm_planner;
pub mod replanner;
pub mod usage;
pub mod validator;

pub use llm_planner::{LlmPlanBackend, LlmPlanner};
pub use replanner::{LlmReplanner, Replanner};
pub use usage::{CostEntry, TokenUsage, UsageLedger};
pub use validator::{extract_json_str, validate_plan};

/// 消息类型再导出：调用方构造 messages 时无需直接依赖 forge-api。
pub use forge_api::ChatMessage;
