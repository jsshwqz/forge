//! 文件写入工具（V4.0+ "写软件"能力基座）。
//!
//! [`WriteFileTool`]：把 LLM/计划产出的代码或文本落到任务工作目录内。
//! 安全约束：仅允许 root 目录内的相对路径（拒绝绝对路径与 `..` 越界）。

use crate::router::{Tool, ToolDescriptor};
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use std::path::{Path, PathBuf};

/// 写文件工具：input = {"path": "...", "content": "..."}，路径限定在 root 内。
pub struct WriteFileTool {
    desc: ToolDescriptor,
    root: PathBuf,
}

impl WriteFileTool {
    /// 以任务工作目录为根创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            desc: ToolDescriptor {
                name: "write_file".into(),
                description: "Write a text file (code/config/docs) into the task workspace. \
                              input: {\"path\":\"relative/path\",\"content\":\"...\"}"
                    .into(),
                input_schema: serde_json::json!({
                    "type":"object",
                    "required":["path","content"],
                    "properties":{
                        "path":{"type":"string"},
                        "content":{"type":"string"}
                    }
                }),
                permission: crate::PermissionLevel::WorkspaceWrite,
            },
            root: Into::<PathBuf>::into(root),
        }
    }

    /// 解析并校验相对路径（拒绝绝对路径 / `..` / 盘符）。
    fn resolve(&self, rel: &str) -> ForgeResult<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute()
            || rel.contains("..")
            || rel.starts_with('/')
            || rel.starts_with('\\')
            || rel.contains(':')
        {
            return Err(ForgeError::InvalidState(format!(
                "write_file: path must be relative inside workspace, got '{rel}'"
            )));
        }
        Ok(self.root.join(p))
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.desc
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let rel = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("write_file: missing 'path'".into()))?;
        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("write_file: missing 'content'".into()))?;

        let full = self.resolve(rel)?;
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ForgeError::InvalidState(format!("write_file mkdir: {e}")))?;
        }
        let bytes = content.len();
        tokio::fs::write(&full, content)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("write_file: {e}")))?;
        Ok(serde_json::json!({ "written": rel, "bytes": bytes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_relative_file_and_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = WriteFileTool::new(tmp.path());
        let out = tool
            .invoke(serde_json::json!({"path":"src/app/main.py","content":"print('hi')"}))
            .await
            .unwrap();
        assert_eq!(out["written"], "src/app/main.py");
        let on_disk = std::fs::read_to_string(tmp.path().join("src/app/main.py")).unwrap();
        assert_eq!(on_disk, "print('hi')");
    }

    #[tokio::test]
    async fn rejects_escape_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = WriteFileTool::new(tmp.path());
        for bad in ["..\\evil.txt", "/abs.txt", "C:\\x.txt", "a/../../b.txt"] {
            assert!(
                tool.invoke(serde_json::json!({"path": bad, "content": "x"})).await.is_err(),
                "must reject {bad}"
            );
        }
        assert!(tool.invoke(serde_json::json!({"path":"a.txt"})).await.is_err());
    }
}
