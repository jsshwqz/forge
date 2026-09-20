//! MCP-003 票3：多 AI 协作记账工具。
//!
//! 通过 MCP 暴露 forge-worklog 的既有能力，让外部 AI 建任务/跑编排后
//! 能就地记账，闭合"多 AI 协作"链条。
//!
//! - forge_worklog_add    → Store::append_record（走 lock + JSON 事实源）
//! - forge_worklog_show   → 读但不过滤展示（轻量，返回最近 N 条）
//! - forge_export         → 读 JSON 渲染三 Markdown 视图（禁手改 MD）
//!
//! 项目根探测：FORGE_PROJECT_ROOT env 优先；否则向上找 AI_WORKFLOW.md。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use forge_worklog::store::Store;
use forge_worklog::{render_handoff, render_progress, render_worklog, RecordKind};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use forge_worklog::StoreError;

/// StoreError → ForgeError 薄转换。
fn map_store_err(e: StoreError) -> ForgeError {
    ForgeError::InvalidState(format!("forge-worklog store: {e}"))
}

/// 自动探测项目根：FORGE_PROJECT_ROOT 优先，否则向上找 AI_WORKFLOW.md。
pub fn detect_project_root() -> ForgeResult<PathBuf> {
    if let Ok(root) = std::env::var("FORGE_PROJECT_ROOT") {
        let p = PathBuf::from(root);
        if p.join("AI_WORKFLOW.md").exists() || p.join("progress.json").exists() {
            return Ok(p);
        }
        return Err(ForgeError::InvalidState(format!(
            "forge-worklog: FORGE_PROJECT_ROOT set but invalid: {}",
            p.display()
        )));
    }
    let mut dir = std::env::current_dir().map_err(|e| {
        ForgeError::InvalidState(format!("forge-worklog: cwd unavailable: {e}"))
    })?;
    loop {
        if dir.join("AI_WORKFLOW.md").exists() || dir.join("progress.json").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(ForgeError::InvalidState(
        "forge-worklog: project root not found (missing AI_WORKFLOW.md/progress.json)".into(),
    ))
}

fn wl_desc(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
        permission: PermissionLevel::WorkspaceWrite,
    }
}

// ── forge_worklog_add ──

pub struct ForgeWorklogAddTool {
    descriptor: ToolDescriptor,
}

impl ForgeWorklogAddTool {
    pub fn new() -> Self {
        Self {
            descriptor: wl_desc(
                "forge_worklog_add",
                "Append a worklog record (R1-R7) to the Forge project JSON store, with cross-process lock.",
                json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["R1","R2","R3","R4","R5","R6","R7"], "description": "Record kind" },
                        "task_id": { "type": "string", "description": "Optional task ID" },
                        "title": { "type": "string", "description": "Record title" },
                        "body": { "type": "string", "description": "Record body (markdown)" }
                    },
                    "required": ["kind", "title", "body"]
                }),
            ),
        }
    }
}

impl Default for ForgeWorklogAddTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ForgeWorklogAddTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let kind_str = input
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("forge_worklog_add: kind required".into()))?;
        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("forge_worklog_add: title required".into()))?;
        let body = input
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ForgeError::InvalidState("forge_worklog_add: body required".into()))?;
        let task_id = input.get("task_id").and_then(|v| v.as_str()).map(|s| s.to_string());

        let kind = match kind_str {
            "R1" => RecordKind::R1Completed,
            "R2" => RecordKind::R2Failed,
            "R3" => RecordKind::R3Blocked,
            "R4" => RecordKind::R4Incomplete,
            "R5" => RecordKind::R5NextActions,
            "R6" => RecordKind::R6Decision,
            "R7" => RecordKind::R7DeviationRisk,
            other => {
                return Err(ForgeError::InvalidState(format!(
                    "forge_worklog_add: bad kind: {other}"
                )))
            }
        };

        let root = detect_project_root()?;
        let store = Store::new(root);
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let record = store
            .append_record(kind, &today, task_id, title, body)
            .map_err(map_store_err)?;
        Ok(json!({ "record_id": record.id }))
    }
}

// ── forge_worklog_show ──

pub struct ForgeWorklogShowTool {
    descriptor: ToolDescriptor,
}

impl ForgeWorklogShowTool {
    pub fn new() -> Self {
        Self {
            descriptor: wl_desc(
                "forge_worklog_show",
                "Show recent worklog records from the Forge project (default last 10).",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Number of records (default 10)" }
                    }
                }),
            ),
        }
    }
}

impl Default for ForgeWorklogShowTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ForgeWorklogShowTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let root = detect_project_root()?;
        let store = Store::new(root);
        let records = store.load_worklog().map_err(map_store_err)?;
        let total = records.len();
        let tail: Vec<Value> = records
            .into_iter()
            .rev()
            .take(limit)
            .map(|r| {
                json!({
                    "id": r.id,
                    "kind": r.kind.code(),
                    "date": r.date,
                    "task_id": r.task_id,
                    "title": r.title,
                })
            })
            .collect();
        Ok(json!({ "total": total, "records": tail }))
    }
}

// ── forge_export ──

pub struct ForgeExportTool {
    descriptor: ToolDescriptor,
}

impl ForgeExportTool {
    pub fn new() -> Self {
        Self {
            descriptor: wl_desc(
                "forge_export",
                "Regenerate PROGRESS.md / WORKLOG.md / HANDOFF.md from JSON sources (never hand-edit MD).",
                json!({ "type": "object", "properties": {} }),
            ),
        }
    }
}

impl Default for ForgeExportTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ForgeExportTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: Value) -> ForgeResult<Value> {
        let root = detect_project_root()?;
        let store = Store::new(root.clone());
        let progress = store.load_progress().map_err(map_store_err)?;
        let records = store.load_worklog().map_err(map_store_err)?;
        let handoff = store.load_handoff().map_err(map_store_err)?;

        for (path, content) in [
            (root.join("PROGRESS.md"), render_progress(&progress)),
            (root.join("WORKLOG.md"), render_worklog(&records)),
            (root.join("HANDOFF.md"), render_handoff(&handoff)),
        ] {
            std::fs::write(&path, content).map_err(|e| {
                ForgeError::InvalidState(format!("forge_export: write {}: {e}", path.display()))
            })?;
        }
        Ok(json!({ "ok": true, "exported": ["PROGRESS.md", "WORKLOG.md", "HANDOFF.md"] }))
    }
}

// ── 构造入口 ──

/// 构造台账工具（白名单点名注册时调用）。
pub fn construct_worklog_tool(name: &str) -> Option<Box<dyn Tool>> {
    match name {
        "forge_worklog_add" => Some(Box::new(ForgeWorklogAddTool::new())),
        "forge_worklog_show" => Some(Box::new(ForgeWorklogShowTool::new())),
        "forge_export" => Some(Box::new(ForgeExportTool::new())),
        _ => None,
    }
}

/// 台账工具名清单。
pub const WORKLOG_TOOLS: &[&str] = &[
    "forge_worklog_add",
    "forge_worklog_show",
    "forge_export",
];

// 供 detect 测试用：允许在测试里以临时目录模拟项目根。
#[allow(dead_code)]
fn is_project_root(dir: &Path) -> bool {
    dir.join("AI_WORKFLOW.md").exists() || dir.join("progress.json").exists()
}