//! forge-recovery：失败分类与恢复引擎。
//!
//! 任何执行失败都先归类，再谈恢复。

pub mod classify;
pub mod engine;

pub use classify::{classify, FailureCategory, FailureRecord};
pub use engine::{
    BoundedRetryStrategy, RecoveryAction, RecoveryEngine, RecoveryStrategy,
};
