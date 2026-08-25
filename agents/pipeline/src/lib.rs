//! forge-pipeline：四角色 Agent 流水线（V3.2，AGENT-P/T/O 系列）。
//!
//! 角色链：Architect(高档模型出计划) → Builder(确定性工具执行) → Tester(Verifier 验收)
//! → Reviewer(高档模型审查) → Gate（Reviewer Reject 一票否决）。
//!
//! - [`role`]：RoleProfile 角色档案（可序列化、可经 forge-cap 注册分发）
//! - [`tier`]：TierRouter 成本分层路由（High/Low 档位 → 模型名；Low 缺省回落 High）
//! - [`reviewer`]：Reviewer Agent（LLM 审查 → Pass/Concern/Reject + 理由）
//! - [`pipeline`]：[`pipeline::run_pipeline`] 四角色编排入口
//!
//! 成本记录（G-V3.2）：每次 LLM 调用的档位/模型/token 数经
//! [`forge_plan_llm::UsageLedger`] 汇总后写进 Session payload。

pub mod pipeline;
pub mod reviewer;
pub mod role;
pub mod tier;

pub use pipeline::{run_pipeline, PipelineDeps, PipelineReport};
pub use reviewer::{
    LlmStepReviewer, ReviewInput, ReviewOutcome, ReviewVerdict, StepReviewer,
};
pub use role::{default_profiles, ModelTier, Role, RoleProfile};
pub use tier::TierRouter;
