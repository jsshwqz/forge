//! 文件写入工具（V4.0+ "写软件"能力基座）。
//!
//! [`WriteFileTool`]：把 LLM/计划产出的代码或文本落到任务工作目录内。
//! 安全约束：仅允许 root 目录内的相对路径（拒绝绝对路径与 `..` 越界）。

use crate::router::{Tool, ToolDescriptor};
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use std::path::{Path, PathBuf};

/// 单次写入上限默认值：1MB（V5.1 WRT-001，env `FORGE_WRITE_MAX_BYTES` 可调）。
pub const FORGE_WRITE_MAX_BYTES_DEFAULT: usize = 1_048_576;

/// 读取写入上限：env `FORGE_WRITE_MAX_BYTES` 优先，非法/未设置回退默认值。
pub(crate) fn write_max_bytes() -> usize {
    std::env::var("FORGE_WRITE_MAX_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(FORGE_WRITE_MAX_BYTES_DEFAULT)
}

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
        // V5.1 WRT-001：resolve 之后、写入之前校验长度上限
        //（规格写 Err(Validation)，本项目无该变体，按 R6-023 适配为 InvalidState）
        if content.len() > write_max_bytes() {
            return Err(ForgeError::InvalidState(
                "write exceeds FORGE_WRITE_MAX_BYTES".into(),
            ));
        }
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

    /// 冻结测试（V5.1 WRT-001）：超 FORGE_WRITE_MAX_BYTES → InvalidState，文件不生成。
    #[tokio::test]
    async fn write_file_size_cap_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = WriteFileTool::new(tmp.path());
        let big = "x".repeat(FORGE_WRITE_MAX_BYTES_DEFAULT + 1);
        let err = tool
            .invoke(serde_json::json!({"path": "big.bin", "content": big}))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("write exceeds FORGE_WRITE_MAX_BYTES"),
            "应报尺寸上限错误: {err}"
        );
        assert!(
            !tmp.path().join("big.bin").exists(),
            "超限文件不得落盘"
        );
    }

    /// 冻结测试（V5.1 WRT-001）：中文内容写入 → 读回逐字节相等。
    #[tokio::test]
    async fn write_file_utf8_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = WriteFileTool::new(tmp.path());
        let content = "你好，世界 — Aion Forge 中文内容 roundtrip ✅";
        tool.invoke(serde_json::json!({"path": "docs/中文.md", "content": content}))
            .await
            .unwrap();
        let on_disk = std::fs::read(tmp.path().join("docs/中文.md")).unwrap();
        assert_eq!(on_disk, content.as_bytes(), "读回必须逐字节相等");
    }
}
