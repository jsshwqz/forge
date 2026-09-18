//! 文件读取工具（V8.0 CTX-001 工作区感知）。
//!
//! [`ReadFileTool`] / [`ListDirTool`]：让模型"看得见"工作区——
//! 读取既有文件内容（带大小上限）与单层目录列举。
//! 防逃逸语义与 WriteFileTool 共用 [`crate::tools_path::resolve_in_root`]（三规则）。
//!
//! 契约来源：build_v80a.md AF-BP-V80A CTX-001。

use crate::router::{Tool, ToolDescriptor};
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use std::path::PathBuf;

/// 单文件读取上限默认值：256KB（V8 CTX-001，env `FORGE_READ_MAX_BYTES` 可调）。
pub const FORGE_READ_MAX_BYTES_DEFAULT: usize = 262_144;

/// 单层目录列举上限：200 条（超出截断 + `truncated:true`）。
pub const FORGE_LIST_MAX_ENTRIES: usize = 200;

/// 读取大小上限：env `FORGE_READ_MAX_BYTES` 优先，非法/未设置回退默认值。
fn read_max_bytes() -> usize {
    std::env::var("FORGE_READ_MAX_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(FORGE_READ_MAX_BYTES_DEFAULT)
}

/// 读文件工具：input = {"path": "相对路径"}，路径限定在 root 内。
pub struct ReadFileTool {
    desc: ToolDescriptor,
    root: PathBuf,
}

impl ReadFileTool {
    /// 以任务工作目录为根创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            desc: ToolDescriptor {
                name: "read_file".into(),
                description: "Read a text file from the task workspace. \
                              input: {\"path\":\"relative/path\"}"
                    .into(),
                input_schema: serde_json::json!({
                    "type":"object",
                    "required":["path"],
                    "properties":{
                        "path":{"type":"string"}
                    }
                }),
                permission: crate::PermissionLevel::ReadOnly,
            },
            root: Into::<PathBuf>::into(root),
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.desc
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let rel = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("read_file: missing 'path'".into()))?;
        let full = crate::tools_path::resolve_in_root(&self.root, rel, "read_file")?;

        // 先查元数据：不存在 → NotFound；超上限 → InvalidState（避免读大文件）。
        let meta = tokio::fs::metadata(&full)
            .await
            .map_err(|e| ForgeError::NotFound(format!("read_file: {rel}: {e}")))?;
        if !meta.is_file() {
            return Err(ForgeError::NotFound(format!(
                "read_file: {rel}: not a regular file"
            )));
        }
        if meta.len() as usize > read_max_bytes() {
            return Err(ForgeError::InvalidState(
                "read exceeds FORGE_READ_MAX_BYTES".into(),
            ));
        }

        let bytes = tokio::fs::read(&full)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("read_file: {e}")))?;
        // 防御性复检（元数据与读取间可能变化）。
        if bytes.len() > read_max_bytes() {
            return Err(ForgeError::InvalidState(
                "read exceeds FORGE_READ_MAX_BYTES".into(),
            ));
        }
        let content = String::from_utf8_lossy(&bytes).to_string();
        Ok(serde_json::json!({ "path": rel, "content": content, "bytes": bytes.len() }))
    }
}

/// 列目录工具：input = {"path"?: 相对目录，缺省根}，单层列举，按名排序。
pub struct ListDirTool {
    desc: ToolDescriptor,
    root: PathBuf,
}

impl ListDirTool {
    /// 以任务工作目录为根创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            desc: ToolDescriptor {
                name: "list_dir".into(),
                description: "List a single directory level in the task workspace. \
                              input: {\"path\"?:\"relative/dir\"}（缺省=根）"
                    .into(),
                input_schema: serde_json::json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"}
                    }
                }),
                permission: crate::PermissionLevel::ReadOnly,
            },
            root: Into::<PathBuf>::into(root),
        }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.desc
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let rel = input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let full = crate::tools_path::resolve_in_root(&self.root, rel, "list_dir")?;

        let mut entries: Vec<serde_json::Value> = Vec::new();
        let mut read = tokio::fs::read_dir(&full)
            .await
            .map_err(|e| ForgeError::NotFound(format!("list_dir: {rel}: {e}")))?;

        let mut names = Vec::new();
        while let Some(entry) = read
            .next_entry()
            .await
            .map_err(|e| ForgeError::InvalidState(format!("list_dir: {e}")))?
        {
            names.push(entry);
        }
        // 按名排序（契约：确定性顺序）。
        names.sort_by_key(|a| a.file_name());

        let mut truncated = false;
        for entry in names {
            if entries.len() >= FORGE_LIST_MAX_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry
                .file_type()
                .await
                .map_err(|e| ForgeError::InvalidState(format!("list_dir entry type: {e}")))?;
            let (kind, bytes) = if ft.is_dir() {
                ("dir", 0u64)
            } else {
                ("file", entry.metadata().await.map(|m| m.len()).unwrap_or(0))
            };
            entries.push(serde_json::json!({
                "name": name,
                "type": kind,
                "bytes": bytes,
            }));
        }

        let mut out = serde_json::Map::new();
        out.insert("entries".into(), serde_json::Value::Array(entries));
        if truncated {
            out.insert("truncated".into(), serde_json::json!(true));
        }
        Ok(serde_json::Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_file_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello 世界").unwrap();
        let tool = ReadFileTool::new(tmp.path());
        let out = tool.invoke(serde_json::json!({"path":"a.txt"})).await.unwrap();
        assert_eq!(out["content"], "hello 世界");
        assert!(out["bytes"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(tmp.path());
        let err = tool.invoke(serde_json::json!({"path":"nope.txt"})).await.unwrap_err();
        assert!(matches!(err, ForgeError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn read_file_escape_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(tmp.path());
        for bad in ["../a.txt", "/abs.txt", "C:\\x.txt"] {
            assert!(tool.invoke(serde_json::json!({"path": bad})).await.is_err(), "must reject {bad}");
        }
    }

    #[tokio::test]
    async fn read_file_size_cap_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("big.bin"), vec![0u8; FORGE_READ_MAX_BYTES_DEFAULT + 1]).unwrap();
        let tool = ReadFileTool::new(tmp.path());
        let err = tool.invoke(serde_json::json!({"path":"big.bin"})).await.unwrap_err();
        assert!(err.to_string().contains("read exceeds FORGE_READ_MAX_BYTES"), "got {err}");
    }

    #[tokio::test]
    async fn list_dir_lists_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
        let tool = ListDirTool::new(tmp.path());
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        let entries = out["entries"].as_array().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
        assert_eq!(entries[2]["type"], "dir");
        assert_eq!(out.get("truncated"), None);
    }
}