//! forge-worklog：多 AI 协作工作记录管理库。
//!
//! 统一用 Rust 结构化类型表示：
//! - 任务状态索引（PROGRESS）
//! - 工作日志（WORKLOG，R1~R7 七类记录）
//! - 交接快照（HANDOFF）
//!
//! 数据存储使用 JSON（serde_json），Markdown 视图由 CLI 生成导出。
//! 对应规范：`AI_WORKFLOW.md`

pub mod export;
pub mod models;
pub mod store;

pub use export::{render_handoff, render_progress, render_worklog};
pub use models::{
    Handoff, NextTask, ProgressEntry, RecordKind, TaskStatus, WorkRecord,
};
pub use store::{Store, StoreError};
