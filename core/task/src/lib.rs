//! forge-task：Task 模型与任务包。
//!
//! 核心是验收标准（AcceptanceCriterion）内建于任务——这是 CPEVR 中 Check 前置的载体。

pub mod model;
pub mod store;

pub use model::{AcceptanceCriterion, CheckSpec, Task, TaskStatus};
pub use store::{InMemoryTaskStore, TaskStore};
