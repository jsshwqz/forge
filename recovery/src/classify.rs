//! 失败分类器：确定性、可枚举的纯映射。

use chrono::{DateTime, Utc};
use forge_core::{ExecutionId, ForgeError, ForgeResult};
use forge_exec::ExecutionStatus;
use serde::{Deserialize, Serialize};

/// 失败类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCategory {
    /// 超时。
    Timeout,
    /// 工具错误。
    ToolError,
    /// 权限拒绝。
    PermissionDenied,
    /// 验证失败。
    VerificationFailed,
    /// 未知。
    Unknown,
}

/// 失败记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureRecord {
    /// 失败 ID，格式为 `fail_uuidv4`。
    pub id: String,
    /// 执行 ID。
    pub execution_id: ExecutionId,
    /// 发生时间。
    pub at: DateTime<Utc>,
    /// 失败类别。
    pub category: FailureCategory,
    /// 失败消息。
    pub message: String,
    /// 是否可重试。
    pub retriable: bool,
}

/// 分类规则（确定性、可枚举）：
/// - `Timeout` → `{Timeout, retriable=true}`
/// - `PermissionDenied` → `{PermissionDenied, false}`
/// - `Failed` → `{ToolError, true}`
/// - `Success` 不得进入本函数（返回 `InvalidState`）
///
/// `Unknown` 分类一律 `retriable = false`（不确定就升级，不盲试）。
pub fn classify(
    execution_id: &ExecutionId,
    status: ExecutionStatus,
    message: &str,
) -> ForgeResult<FailureRecord> {
    let (category, retriable) = match status {
        ExecutionStatus::Timeout => (FailureCategory::Timeout, true),
        ExecutionStatus::PermissionDenied => (FailureCategory::PermissionDenied, false),
        ExecutionStatus::Failed => (FailureCategory::ToolError, true),
        ExecutionStatus::Success => {
            return Err(ForgeError::InvalidState(
                "cannot classify Success as failure".into(),
            ));
        }
    };

    Ok(FailureRecord {
        id: format!("fail_{}", uuid::Uuid::new_v4()),
        execution_id: execution_id.clone(),
        at: Utc::now(),
        category,
        message: message.to_string(),
        retriable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_timeout() {
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::Timeout, "timed out").unwrap();
        assert_eq!(record.category, FailureCategory::Timeout);
        assert!(record.retriable);
    }

    #[test]
    fn test_classify_permission_denied() {
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::PermissionDenied, "no access").unwrap();
        assert_eq!(record.category, FailureCategory::PermissionDenied);
        assert!(!record.retriable);
    }

    #[test]
    fn test_classify_failed() {
        let eid = ExecutionId::new_execution_id();
        let record = classify(&eid, ExecutionStatus::Failed, "tool error").unwrap();
        assert_eq!(record.category, FailureCategory::ToolError);
        assert!(record.retriable);
    }

    #[test]
    fn test_classify_success_errors() {
        let eid = ExecutionId::new_execution_id();
        let result = classify(&eid, ExecutionStatus::Success, "should not happen");
        assert!(result.is_err());
    }

    #[test]
    fn test_deterministic_same_input_same_output() {
        let eid = ExecutionId::new_execution_id();
        let r1 = classify(&eid, ExecutionStatus::Timeout, "msg").unwrap();
        let r2 = classify(&eid, ExecutionStatus::Timeout, "msg").unwrap();
        assert_eq!(r1.category, r2.category);
        assert_eq!(r1.retriable, r2.retriable);
        assert_eq!(r1.message, r2.message);
    }
}
