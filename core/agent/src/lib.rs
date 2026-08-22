//! forge-agent：Agent 抽象与动作空间。
//!
//! 冻结 Agent 接口：配置、角色、动作空间。
//! 后续所有 Agent（Builder/Tester/Reviewer）都是该 trait 的实现。

pub mod model;

pub use model::{
    Agent, AgentAction, AgentConfig, AgentOutcome, AgentRole, TurnInput,
};
