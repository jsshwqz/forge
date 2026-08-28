//! 统一 ID 类型模块。
//!
//! 每种实体拥有独立的强类型 ID，格式为 `前缀_uuidv4`（如 `task_3f2c...`）。
//! 所有 ID 类型满足：Clone / Debug / PartialEq / Eq / Hash / Serialize / Deserialize / Display。

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// 用于生成带前缀的强类型 ID。
macro_rules! id_types {
    ($($name:ident: $prefix:expr, $ctor:ident),* $(,)?) => {
        $(
            /// 强类型 ID，格式为 `前缀_uuidv4`。
            #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
            pub struct $name(pub String);

            impl $name {
                /// 生成新的唯一 ID。
                pub fn $ctor() -> Self {
                    $ctor()
                }
            }

            /// 生成新的唯一 ID。
            pub fn $ctor() -> $name {
                $name(format!("{}_{}", $prefix, Uuid::new_v4()))
            }

            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&self.0)
                }
            }

            impl From<String> for $name {
                fn from(s: String) -> Self {
                    Self(s)
                }
            }

            impl AsRef<str> for $name {
                fn as_ref(&self) -> &str {
                    &self.0
                }
            }
        )*
    };
}

id_types! {
    TaskId:       "task",       new_task_id,
    AgentId:      "agent",      new_agent_id,
    PlanId:       "plan",       new_plan_id,
    SessionId:    "session",    new_session_id,
    ExecutionId:  "execution",  new_execution_id,
    CapabilityId: "capability", new_capability_id,
    ArtifactId:   "artifact",   new_artifact_id,
    EvidenceId:   "evidence",   new_evidence_id,
    ProductId:    "product",    new_product_id,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_uniqueness() {
        let a = TaskId::new_task_id();
        let b = TaskId::new_task_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_task_id_prefix() {
        let id = TaskId::new_task_id();
        assert!(id.0.starts_with("task_"));
    }

    #[test]
    fn test_agent_id_prefix() {
        let id = AgentId::new_agent_id();
        assert!(id.0.starts_with("agent_"));
    }

    #[test]
    fn test_plan_id_prefix() {
        let id = PlanId::new_plan_id();
        assert!(id.0.starts_with("plan_"));
    }

    #[test]
    fn test_session_id_prefix() {
        let id = SessionId::new_session_id();
        assert!(id.0.starts_with("session_"));
    }

    #[test]
    fn test_execution_id_prefix() {
        let id = ExecutionId::new_execution_id();
        assert!(id.0.starts_with("execution_"));
    }

    #[test]
    fn test_capability_id_prefix() {
        let id = CapabilityId::new_capability_id();
        assert!(id.0.starts_with("capability_"));
    }

    #[test]
    fn test_artifact_id_prefix() {
        let id = ArtifactId::new_artifact_id();
        assert!(id.0.starts_with("artifact_"));
    }

    #[test]
    fn test_evidence_id_prefix() {
        let id = EvidenceId::new_evidence_id();
        assert!(id.0.starts_with("evidence_"));
    }

    #[test]
    fn test_product_id_prefix() {
        let id = ProductId::new_product_id();
        assert!(id.0.starts_with("product_"));
    }

    #[test]
    fn test_task_id_serde_roundtrip() {
        let id = TaskId::new_task_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_session_id_serde_roundtrip() {
        let id = SessionId::new_session_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_agent_id_serde_roundtrip() {
        let id = AgentId::new_agent_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: AgentId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_execution_id_serde_roundtrip() {
        let id = ExecutionId::new_execution_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: ExecutionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_artifact_id_serde_roundtrip() {
        let id = ArtifactId::new_artifact_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: ArtifactId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_evidence_id_serde_roundtrip() {
        let id = EvidenceId::new_evidence_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: EvidenceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_product_id_serde_roundtrip() {
        let id = ProductId::new_product_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: ProductId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_capability_id_serde_roundtrip() {
        let id = CapabilityId::new_capability_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: CapabilityId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_plan_id_serde_roundtrip() {
        let id = PlanId::new_plan_id();
        let json = serde_json::to_string(&id).unwrap();
        let back: PlanId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_display() {
        let id = TaskId::from("task_abc123".to_string());
        assert_eq!(id.to_string(), "task_abc123");
    }

    #[test]
    fn test_as_ref() {
        let id = SessionId::from("session_xyz".to_string());
        assert_eq!(id.as_ref(), "session_xyz");
    }
}


/// 租户 ID（前缀 ten_）
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TenantId(String);

impl TenantId {
    pub fn new() -> Self {
        Self(format!("ten_{}", uuid::Uuid::new_v4()))
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TenantId {
    fn from(s: String) -> Self { Self(s) }
}

impl From<&str> for TenantId {
    fn from(s: &str) -> Self { Self(s.into()) }
}

/// 默认租户 ID
pub const DEFAULT_TENANT_ID: &str = "default";
