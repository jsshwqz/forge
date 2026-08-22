//! forge-core：Aion Forge 2.0 基础类型库。
//!
//! 提供 ID 类型、统一错误类型与别名，供全工作区所有 crate 复用。

pub mod error;
pub mod id;

pub use error::{ForgeError, ForgeResult};
pub use id::{
    new_agent_id, new_artifact_id, new_capability_id, new_evidence_id, new_execution_id,
    new_plan_id, new_product_id, new_session_id, new_task_id, AgentId, ArtifactId, CapabilityId,
    EvidenceId, ExecutionId, PlanId, ProductId, SessionId, TaskId,
};
