//! Forge 文件系统工具：read_file / write_file / edit_file / exec_command。
//!
//! 暴露 `register_all(router)` 统一注册 4 个工具。

mod tool;

pub use tool::{
    register_all, EditFileTool, ExecCommandTool, ReadFileTool, WriteFileTool,
};
