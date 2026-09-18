//! 增量编辑工具（V8.0 EDIT-001）。
//!
//! [`EditPatchTool`]：对工作区内既有文件做精确串替换，不碰无关内容。
//! 与 WriteFileTool 共用防逃逸三规则（[`crate::tools_path::resolve_in_root`]）
//! 与写入上限（[`crate::tools_file::write_max_bytes`]）。
//!
//! 契约来源：build_v80a.md AF-BP-V80A EDIT-001。

use crate::router::{Tool, ToolDescriptor};
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use std::path::PathBuf;

/// 增量编辑工具：input = {"path", "edits":[{find,replace,replace_all?}], "create_if_missing"?}
pub struct EditPatchTool {
    desc: ToolDescriptor,
    root: PathBuf,
}

impl EditPatchTool {
    /// 以任务工作目录为根创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            desc: ToolDescriptor {
                name: "edit_patch".into(),
                description: "Apply exact string replacements to an existing file in the task \
                              workspace (incremental edit). input: {\"path\":\"relative/path\",\
                              \"edits\":[{\"find\":\"...\",\"replace\":\"...\",\"replace_all\"?}]\
                              ,\"create_if_missing\"?}"
                    .into(),
                input_schema: serde_json::json!({
                    "type":"object",
                    "required":["path","edits"],
                    "properties":{
                        "path":{"type":"string"},
                        "edits":{
                            "type":"array",
                            "items":{
                                "type":"object",
                                "required":["find","replace"],
                                "properties":{
                                    "find":{"type":"string"},
                                    "replace":{"type":"string"},
                                    "replace_all":{"type":"boolean"}
                                }
                            }
                        },
                        "create_if_missing":{"type":"boolean"}
                    }
                }),
                permission: crate::PermissionLevel::WorkspaceWrite,
            },
            root: Into::<PathBuf>::into(root),
        }
    }
}

#[async_trait]
impl Tool for EditPatchTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.desc
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let rel = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("edit_patch: missing 'path'".into()))?;
        let edits = input
            .get("edits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ForgeError::InvalidState("edit_patch: missing 'edits'".into()))?;
        let create_if_missing = input
            .get("create_if_missing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let full = crate::tools_path::resolve_in_root(&self.root, rel, "edit_patch")?;

        // V8 M4：符号链接逃逸校验（存在时 canonicalize 后仍在 root 内）。
        let full = if full.exists() {
            crate::tools_path::ensure_within_root(&self.root, &full, "edit_patch")?
        } else {
            full
        };

        // 读入现有内容；缺文件时按 create_if_missing 决定空起或 NotFound。
        let mut content = if full.exists() {
            tokio::fs::read_to_string(&full)
                .await
                .map_err(|e| ForgeError::InvalidState(format!("edit_patch read: {e}")))?
        } else if create_if_missing {
            String::new()
        } else {
            return Err(ForgeError::NotFound(format!("edit_patch: {rel}: not found")));
        };

        // R1：逐条顺序应用，前一条输出是后一条输入。
        let mut applied = 0usize;
        for edit in edits {
            let find = edit
                .get("find")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ForgeError::InvalidState("edit_patch: edit missing 'find'".into()))?;
            let replace = edit
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ForgeError::InvalidState("edit_patch: edit missing 'replace'".into()))?;
            let replace_all = edit
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if find.is_empty() {
                return Err(ForgeError::InvalidState(
                    "edit: empty find is not allowed".into(),
                ));
            }
            if !content.contains(find) {
                return Err(ForgeError::InvalidState("edit: find not found".into()));
            }
            // 重叠命中计数（match_indices），确保 "aa" in "aaa" 正确识别为多处。
            let occurrences = content.match_indices(find).count();
            if occurrences > 1 && !replace_all {
                return Err(ForgeError::InvalidState(
                    "edit: find not unique, use replace_all".into(),
                ));
            }
            content = if replace_all {
                content.replace(find, replace)
            } else {
                content.replacen(find, replace, 1)
            };
            applied += 1;
        }

        // R2：写入上限沿用 FORGE_WRITE_MAX_BYTES。
        if content.len() > crate::tools_file::write_max_bytes() {
            return Err(ForgeError::InvalidState(
                "edit exceeds FORGE_WRITE_MAX_BYTES".into(),
            ));
        }
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ForgeError::InvalidState(format!("edit_patch mkdir: {e}")))?;
        }
        tokio::fs::write(&full, &content)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("edit_patch write: {e}")))?;

        Ok(serde_json::json!({ "path": rel, "applied": applied }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edit_patch_unique_replace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello world").unwrap();
        let tool = EditPatchTool::new(tmp.path());
        let out = tool
            .invoke(serde_json::json!({
                "path": "a.txt",
                "edits": [{"find": "world", "replace": "forge"}]
            }))
            .await
            .unwrap();
        assert_eq!(out["applied"], 1);
        assert_eq!(std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "hello forge");
    }

    #[tokio::test]
    async fn edit_patch_sequential_apply() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "aaa bbb").unwrap();
        let tool = EditPatchTool::new(tmp.path());
        tool.invoke(serde_json::json!({
            "path": "a.txt",
            "edits": [
                {"find": "aaa", "replace": "111"},
                {"find": "bbb", "replace": "222"}
            ]
        }))
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "111 222");
    }

    #[tokio::test]
    async fn edit_patch_find_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        let tool = EditPatchTool::new(tmp.path());
        let err = tool
            .invoke(serde_json::json!({"path": "a.txt", "edits": [{"find": "nope", "replace": "x"}]}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("find not found"), "got {err}");
    }

    #[tokio::test]
    async fn edit_patch_not_unique_requires_flag() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x x x").unwrap();
        let tool = EditPatchTool::new(tmp.path());
        let err = tool
            .invoke(serde_json::json!({"path": "a.txt", "edits": [{"find": "x", "replace": "y"}]}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not unique"), "got {err}");
        // replace_all:true 应成功
        tool.invoke(serde_json::json!({
            "path": "a.txt",
            "edits": [{"find": "x", "replace": "y", "replace_all": true}]
        }))
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "y y y");
    }

    #[tokio::test]
    async fn edit_patch_create_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = EditPatchTool::new(tmp.path());
        // 缺省：文件不存在 → NotFound
        let err = tool
            .invoke(serde_json::json!({"path": "new.txt", "edits": [{"find": "a", "replace": "b"}]}))
            .await
            .unwrap_err();
        assert!(matches!(err, ForgeError::NotFound(_)));
        // create_if_missing: 空文件起，但 find 不存在仍报错
        let err2 = tool
            .invoke(serde_json::json!({
                "path": "new.txt", "create_if_missing": true,
                "edits": [{"find": "a", "replace": "b"}]
            }))
            .await
            .unwrap_err();
        assert!(err2.to_string().contains("find not found"));
    }

    #[tokio::test]
    async fn edit_patch_escape_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = EditPatchTool::new(tmp.path());
        for bad in ["../a.txt", "/abs.txt", "C:\\x.txt"] {
            assert!(
                tool.invoke(serde_json::json!({"path": bad, "edits": [{"find":"a","replace":"b"}]}))
                    .await
                    .is_err(),
                "must reject {bad}"
            );
        }
    }
}
#[cfg(unix)]
#[tokio::test]
async fn edit_patch_symlink_escape_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("victim.txt"), "hello").unwrap();
    std::os::unix::fs::symlink(outside.path().join("victim.txt"), tmp.path().join("link.txt")).unwrap();
    let tool = EditPatchTool::new(tmp.path());
    let err = tool
        .invoke(serde_json::json!({
            "path": "link.txt",
            "edits": [{"find": "hello", "replace": "pwned"}]
        }))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ForgeError::PermissionDenied(_)),
        "符号链接逃逸必须 PermissionDenied，got {err:?}"
    );
    // 外部文件必须未被篡改。
    assert_eq!(
        std::fs::read_to_string(outside.path().join("victim.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn edit_patch_empty_find_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
    let tool = EditPatchTool::new(tmp.path());
    let err = tool
        .invoke(serde_json::json!({"path": "a.txt", "edits": [{"find": "", "replace": "x"}]}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("empty find"), "got {err}");
}
