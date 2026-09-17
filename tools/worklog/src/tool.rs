//! forge-exec Tool 适配层：把 worklog 的 Store/导出能力包装成 MCP 可注册工具。
//!
//! 提供三个工具：
//! - `worklog_status`（ReadOnly）：读取 progress/worklog/handoff 摘要，不写任何文件
//! - `worklog_append`（WorkspaceWrite）：追加一条工作记录（R1~R7 分类）
//! - `worklog_export`（ReadOnly）：渲染 Markdown 视图（progress/worklog/handoff）
//!
//! 设计约束：
//! - 根目录默认取当前工作目录，可用 input.root 覆盖；
//! - 每个工具独立实现 Tool trait，descriptor 的 input_schema 严格对应实际入参；
//! - append 遵守 Store 的并发契约：读改写全程持有 `Store::lock`。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use std::path::PathBuf;

use crate::export;
use crate::models::RecordKind;
use crate::store::{Store, StoreError};

/// 把 StoreError 映射为 ForgeError。
fn store_err(e: StoreError) -> ForgeError {
    match e {
        StoreError::Io(err) => ForgeError::Io(err),
        StoreError::Json(err) => ForgeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("json error: {err}"),
        )),
        StoreError::Invalid(msg) => ForgeError::InvalidState(msg),
    }
}

/// 解析根目录：input.root > 当前工作目录。
fn resolve_root(input: &serde_json::Value) -> Result<PathBuf, ForgeError> {
    if let Some(root) = input.get("root").and_then(|v| v.as_str()) {
        if root.is_empty() {
            return Err(ForgeError::InvalidState("root 不能为空字符串".into()));
        }
        Ok(PathBuf::from(root))
    } else {
        std::env::current_dir().map_err(|e| {
            ForgeError::InvalidState(format!("无法获取当前工作目录: {e}"))
        })
    }
}

/// 解析 RecordKind：接受 R1..R7 代码或 label。
fn parse_record_kind(s: &str) -> Result<RecordKind, ForgeError> {
    for k in [
        RecordKind::R1Completed,
        RecordKind::R2Failed,
        RecordKind::R3Blocked,
        RecordKind::R4Incomplete,
        RecordKind::R5NextActions,
        RecordKind::R6Decision,
        RecordKind::R7DeviationRisk,
    ] {
        if k.code() == s || k.label() == s {
            return Ok(k);
        }
    }
    Err(ForgeError::InvalidState(format!(
        "未知 RecordKind: {s}（合法值：R1..R7 或其 label）"
    )))
}

// ---------- worklog_status ----------

/// 只读工具：返回 progress/worklog/handoff 的摘要 JSON。
pub struct WorklogStatusTool {
    descriptor: ToolDescriptor,
}

impl WorklogStatusTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "worklog_status".into(),
                description: "读取项目 worklog 状态（progress/worklog/handoff 摘要）。只读，不写文件。可选 root 指定项目根目录。"
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "项目根目录（默认当前工作目录）" }
                    }
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for WorklogStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorklogStatusTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let root = resolve_root(&input)?;
        let store = Store::new(root);
        let progress = store.load_progress().map_err(store_err)?;
        let worklog = store.load_worklog().map_err(store_err)?;
        let handoff = store.load_handoff().map_err(store_err)?;

        // 状态分布统计
        let mut status_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for e in &progress {
            *status_counts.entry(e.status.label().to_string()).or_insert(0) += 1;
        }

        Ok(serde_json::json!({
            "progress": {
                "total": progress.len(),
                "by_status": status_counts,
                "entries": progress
            },
            "worklog": {
                "total": worklog.len(),
                "latest": worklog.iter().rev().take(5).collect::<Vec<_>>()
            },
            "handoff": {
                "updated_at": handoff.updated_at,
                "current_status": handoff.current_status,
                "blockers": handoff.blockers,
                "next_tasks": handoff.next_tasks,
                "risks": handoff.risks,
                "advice": handoff.advice
            }
        }))
    }
}

// ---------- worklog_append ----------

/// 写工作区工具：追加一条工作记录。
pub struct WorklogAppendTool {
    descriptor: ToolDescriptor,
}

impl WorklogAppendTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "worklog_append".into(),
                description: "追加一条 worklog 工作记录（R1 完成 / R2 失败 / R3 阻塞 / R4 决策 / R5 审查 / R6 恢复 / R7 学习）。写入 worklog.json。"
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "项目根目录（默认当前工作目录）" },
                        "kind": { "type": "string", "enum": ["R1","R2","R3","R4","R5","R6","R7"], "description": "记录分类" },
                        "date": { "type": "string", "description": "记录日期（YYYY-MM-DD）" },
                        "task_id": { "type": "string", "description": "关联任务 ID（可选）" },
                        "title": { "type": "string", "description": "记录标题" },
                        "body": { "type": "string", "description": "记录正文" }
                    },
                    "required": ["kind", "date", "title", "body"]
                }),
                permission: PermissionLevel::WorkspaceWrite,
            },
        }
    }
}

impl Default for WorklogAppendTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorklogAppendTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let root = resolve_root(&input)?;
        let kind_str = input
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("缺少必填字段 kind".into()))?;
        let date = input
            .get("date")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("缺少必填字段 date".into()))?;
        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("缺少必填字段 title".into()))?;
        let body = input
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("缺少必填字段 body".into()))?;
        let task_id = input.get("task_id").and_then(|v| v.as_str()).map(String::from);

        let kind = parse_record_kind(kind_str)?;
        let store = Store::new(root);

        // 读改写全程持锁（Store 并发契约）
        let _guard = store.lock().map_err(store_err)?;
        let record = store
            .append_record(kind, date, task_id, title, body)
            .map_err(store_err)?;

        Ok(serde_json::json!({
            "ok": true,
            "record_id": record.id,
            "kind": record.kind.code(),
            "date": record.date,
            "task_id": record.task_id,
            "title": record.title
        }))
    }
}

// ---------- worklog_export ----------

/// 只读工具：渲染 Markdown 视图（不落盘，只返回文本）。
pub struct WorklogExportTool {
    descriptor: ToolDescriptor,
}

impl WorklogExportTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "worklog_export".into(),
                description: "将 worklog 的 progress/worklog/handoff 渲染为 Markdown 视图（只读，不落盘）。"
                    .into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "项目根目录（默认当前工作目录）" },
                        "view": { "type": "string", "enum": ["progress","worklog","handoff","all"], "description": "导出视图（默认 all）" }
                    }
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for WorklogExportTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorklogExportTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let root = resolve_root(&input)?;
        let view = input.get("view").and_then(|v| v.as_str()).unwrap_or("all");
        let store = Store::new(root);

        let mut out = String::new();
        match view {
            "progress" => {
                let entries = store.load_progress().map_err(store_err)?;
                out.push_str(&export::render_progress(&entries));
            }
            "worklog" => {
                let records = store.load_worklog().map_err(store_err)?;
                out.push_str(&export::render_worklog(&records));
            }
            "handoff" => {
                let h = store.load_handoff().map_err(store_err)?;
                out.push_str(&export::render_handoff(&h));
            }
            _ => {
                let entries = store.load_progress().map_err(store_err)?;
                let records = store.load_worklog().map_err(store_err)?;
                let h = store.load_handoff().map_err(store_err)?;
                out.push_str(&export::render_progress(&entries));
                out.push('\n');
                out.push_str(&export::render_worklog(&records));
                out.push('\n');
                out.push_str(&export::render_handoff(&h));
            }
        }

        Ok(serde_json::json!({
            "view": view,
            "markdown": out
        }))
    }
}

// ---------- 便捷注册 ----------

/// 批量注册全部 worklog 工具到路由器。
pub fn register_all(router: &forge_exec::ToolRouter) -> forge_core::ForgeResult<()> {
    router.register(Box::new(WorklogStatusTool::new()))?;
    router.register(Box::new(WorklogAppendTool::new()))?;
    router.register(Box::new(WorklogExportTool::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptors() {
        let s = WorklogStatusTool::new();
        let a = WorklogAppendTool::new();
        let e = WorklogExportTool::new();
        assert_eq!(s.descriptor().name, "worklog_status");
        assert_eq!(s.descriptor().permission, PermissionLevel::ReadOnly);
        assert_eq!(a.descriptor().name, "worklog_append");
        assert_eq!(a.descriptor().permission, PermissionLevel::WorkspaceWrite);
        assert_eq!(e.descriptor().name, "worklog_export");
        assert_eq!(e.descriptor().permission, PermissionLevel::ReadOnly);
    }

    #[test]
    fn test_parse_record_kind() {
        assert_eq!(parse_record_kind("R1").unwrap(), RecordKind::R1Completed);
        assert!(parse_record_kind("R99").is_err());
    }

    #[tokio::test]
    async fn test_status_empty() {
        let tool = WorklogStatusTool::new();
        let tmp = tempfile::tempdir().unwrap();
        let r = tool
            .invoke(serde_json::json!({ "root": tmp.path().to_str().unwrap() }))
            .await
            .unwrap();
        assert_eq!(r["progress"]["total"], 0);
    }

    #[tokio::test]
    async fn test_append_and_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let a = WorklogAppendTool::new();
        let r = a
            .invoke(serde_json::json!({
                "root": root,
                "kind": "R1",
                "date": "2026-07-11",
                "task_id": "TASK-001",
                "title": "完成",
                "body": "详情"
            }))
            .await
            .unwrap();
        assert_eq!(r["record_id"], "R1-001");

        let s = WorklogStatusTool::new();
        let r2 = s
            .invoke(serde_json::json!({ "root": root }))
            .await
            .unwrap();
        assert_eq!(r2["worklog"]["total"], 1);
    }

    #[tokio::test]
    async fn test_export_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let e = WorklogExportTool::new();
        let r = e
            .invoke(serde_json::json!({ "root": root, "view": "all" }))
            .await
            .unwrap();
        assert!(r["markdown"].as_str().unwrap().contains("任务状态索引"));
    }
}
