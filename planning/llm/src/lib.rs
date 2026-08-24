//! forge-plan-llm：LLM 驱动规划 + 重规划（V3.1）。
//!
//! - [`validator::validate_plan`]：校验 LLM 原始 JSON → 合法 Plan（纯函数）
//! - [`LlmPlanner`]：实现 Planner trait，prompt→LLM→校验→repair 循环
//! - [`Replanner`]：原计划+失败记录 → 修订计划

pub mod llm_planner;
pub mod replanner;
pub mod validator;

pub use llm_planner::LlmPlanner;
pub use replanner::LlmReplanner;
pub use validator::validate_plan;
