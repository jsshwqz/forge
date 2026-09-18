//! forge-exec：执行运行时。
//!
//! 工具抽象、路由器、四级权限模型与执行引擎。
//! `PermissionPolicy` trait 定义在此 crate 中，供 forge-sandbox 实现。

pub mod dispatcher;
pub mod engine;
pub mod permission_level;
pub mod policy;
pub mod router;
pub mod tools_file;
pub mod tools_path;
pub mod tools_read;
pub mod tools_edit;

pub use dispatcher::EngineDispatcher;
pub use engine::{ExecutionEngine, ExecutionRequest, ExecutionResult, ExecutionStatus};
pub use permission_level::PermissionLevel;
pub use policy::{PermissionPolicy, PolicyContext};
pub use router::{EchoTool, Tool, ToolDescriptor, ToolRouter};
pub use tools_file::WriteFileTool;
pub use tools_path::{ensure_within_root, resolve_in_root};
pub use tools_read::{ListDirTool, ReadFileTool, FORGE_READ_MAX_BYTES_DEFAULT};
pub use tools_edit::EditPatchTool;
