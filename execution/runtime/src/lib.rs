//! forge-exec：执行运行时。
//!
//! 工具抽象、路由器、四级权限模型与执行引擎。
//! `PermissionPolicy` trait 定义在此 crate 中，供 forge-sandbox 实现。

pub mod dispatcher;
pub mod engine;
pub mod permission_level;
pub mod policy;
pub mod router;

pub use dispatcher::EngineDispatcher;
pub use engine::{ExecutionEngine, ExecutionRequest, ExecutionResult, ExecutionStatus};
pub use permission_level::PermissionLevel;
pub use policy::{PermissionPolicy, PolicyContext};
pub use router::{EchoTool, Tool, ToolDescriptor, ToolRouter};
