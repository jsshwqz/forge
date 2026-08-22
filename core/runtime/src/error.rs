//! 统一错误类型模块。
//!
//! 所有 crate 共享 `ForgeError` 和 `ForgeResult`，避免各模块自行定义错误类型。

use thiserror::Error;

/// 全局错误类型，涵盖 Aion Forge 2.0 第一阶段所有错误场景。
#[derive(Debug, Error)]
pub enum ForgeError {
    /// 资源未找到。
    #[error("not found: {0}")]
    NotFound(String),

    /// 状态非法或不允许的状态迁移。
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// 依赖缺失（如引用了不存在的实体）。
    #[error("dependency missing: {0}")]
    DependencyMissing(String),

    /// 权限拒绝。
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// 验证失败。
    #[error("verification failed: {0}")]
    VerificationFailed(String),

    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 全局 Result 别名。
pub type ForgeResult<T> = Result<T, ForgeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_found_display() {
        let e = ForgeError::NotFound("session abc".into());
        assert_eq!(e.to_string(), "not found: session abc");
    }

    #[test]
    fn test_invalid_state_display() {
        let e = ForgeError::InvalidState("illegal transition".into());
        assert_eq!(e.to_string(), "invalid state: illegal transition");
    }

    #[test]
    fn test_dependency_missing_display() {
        let e = ForgeError::DependencyMissing("forge-core".into());
        assert_eq!(e.to_string(), "dependency missing: forge-core");
    }

    #[test]
    fn test_permission_denied_display() {
        let e = ForgeError::PermissionDenied("write access".into());
        assert_eq!(e.to_string(), "permission denied: write access");
    }

    #[test]
    fn test_verification_failed_display() {
        let e = ForgeError::VerificationFailed("test AC-1".into());
        assert_eq!(e.to_string(), "verification failed: test AC-1");
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let e: ForgeError = io_err.into();
        assert!(e.to_string().contains("io error"));
        assert!(e.to_string().contains("file gone"));
    }
}
