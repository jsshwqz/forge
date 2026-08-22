//! forge-exec：执行运行时。
//!
//! 工具抽象、路由器和四级权限模型。
//! 权限级别在本任务冻结，供 EXEC-003 的策略引擎消费。

pub mod permission_level;
pub mod router;

pub use permission_level::PermissionLevel;
pub use router::{EchoTool, Tool, ToolDescriptor, ToolRouter};
