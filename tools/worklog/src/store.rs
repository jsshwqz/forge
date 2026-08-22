//! JSON 存储：PROGRESS / WORKLOG / HANDOFF 的读写。
//!
//! 数据文件默认放在项目根目录（与 AI_WORKFLOW.md 同级）：
//! - progress.json
//! - worklog.json
//! - handoff.json
//!
//! Markdown 视图（PROGRESS.md / WORKLOG.md / HANDOFF.md）由 CLI 从 JSON 导出生成，
//! 避免手工维护两个事实源。

use crate::models::{Handoff, ProgressEntry, WorkRecord};
use std::fs;
use std::path::{Path, PathBuf};

/// 存储错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid data: {0}")]
    Invalid(String),
}

/// 项目状态存储。
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// 创建存储，root 为项目根目录。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 返回 progress.json 路径。
    pub fn progress_path(&self) -> PathBuf {
        self.root.join("progress.json")
    }

    /// 返回 worklog.json 路径。
    pub fn worklog_path(&self) -> PathBuf {
        self.root.join("worklog.json")
    }

    /// 返回 handoff.json 路径。
    pub fn handoff_path(&self) -> PathBuf {
        self.root.join("handoff.json")
    }

    // ---- PROGRESS ----

    /// 读取任务状态索引；文件不存在时返回空列表。
    pub fn load_progress(&self) -> Result<Vec<ProgressEntry>, StoreError> {
        let path = self.progress_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// 保存任务状态索引。
    pub fn save_progress(&self, entries: &[ProgressEntry]) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(entries)?;
        fs::write(self.progress_path(), json)?;
        Ok(())
    }

    /// 按 task_id 查找进度条目。
    pub fn find_progress(
        &self,
        task_id: &str,
    ) -> Result<Option<ProgressEntry>, StoreError> {
        Ok(self
            .load_progress()?
            .into_iter()
            .find(|e| e.task_id == task_id))
    }

    // ---- WORKLOG ----

    /// 读取工作日志；文件不存在时返回空列表。
    pub fn load_worklog(&self) -> Result<Vec<WorkRecord>, StoreError> {
        let path = self.worklog_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// 保存工作日志。
    pub fn save_worklog(&self, records: &[WorkRecord]) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(records)?;
        fs::write(self.worklog_path(), json)?;
        Ok(())
    }

    /// 追加一条工作记录，自动分配 ID（R<kind>-NNN）。
    pub fn append_record(
        &self,
        kind: crate::models::RecordKind,
        date: &str,
        task_id: Option<String>,
        title: &str,
        body: &str,
    ) -> Result<WorkRecord, StoreError> {
        let mut records = self.load_worklog()?;
        // 计算下一个序号
        let prefix = format!("{}-", kind.code());
        let next_seq = records
            .iter()
            .filter(|r| r.id.starts_with(&prefix))
            .filter_map(|r| {
                r.id
                    .strip_prefix(&prefix)
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;

        let record = WorkRecord {
            id: format!("{}{:03}", prefix, next_seq),
            kind,
            date: date.to_string(),
            task_id,
            title: title.to_string(),
            body: body.to_string(),
        };

        records.push(record.clone());
        self.save_worklog(&records)?;
        Ok(record)
    }

    // ---- HANDOFF ----

    /// 读取交接快照；文件不存在时返回默认空快照。
    pub fn load_handoff(&self) -> Result<Handoff, StoreError> {
        let path = self.handoff_path();
        if !path.exists() {
            return Ok(Handoff::default());
        }
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// 保存交接快照。
    pub fn save_handoff(&self, handoff: &Handoff) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(handoff)?;
        fs::write(self.handoff_path(), json)?;
        Ok(())
    }
}

/// 确保父目录存在。
pub fn ensure_dir(path: &Path) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RecordKind, TaskStatus};

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        (dir, store)
    }

    #[test]
    fn test_progress_roundtrip() {
        let (_dir, store) = temp_store();
        let entries = vec![ProgressEntry {
            task_id: "TASK-001".into(),
            name: "测试".into(),
            status: TaskStatus::Wip,
            owner: Some("builder-a".into()),
            last_record: None,
            commit: None,
        }];
        store.save_progress(&entries).unwrap();
        let loaded = store.load_progress().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].task_id, "TASK-001");
        assert_eq!(loaded[0].status, TaskStatus::Wip);
    }

    #[test]
    fn test_load_missing_returns_empty() {
        let (_dir, store) = temp_store();
        assert!(store.load_progress().unwrap().is_empty());
        assert!(store.load_worklog().unwrap().is_empty());
    }

    #[test]
    fn test_append_record_auto_ids() {
        let (_dir, store) = temp_store();
        let r1 = store
            .append_record(RecordKind::R1Completed, "2026-08-22", Some("TASK-001".into()), "a", "body")
            .unwrap();
        let r2 = store
            .append_record(RecordKind::R1Completed, "2026-08-22", None, "b", "body")
            .unwrap();
        assert_eq!(r1.id, "R1-001");
        assert_eq!(r2.id, "R1-002");
    }

    #[test]
    fn test_append_record_kind_specific_seq() {
        let (_dir, store) = temp_store();
        store
            .append_record(RecordKind::R1Completed, "2026-08-22", None, "a", "body")
            .unwrap();
        let r2 = store
            .append_record(RecordKind::R2Failed, "2026-08-22", None, "b", "body")
            .unwrap();
        assert_eq!(r2.id, "R2-001"); // R2 从 001 重新计数
    }

    #[test]
    fn test_handoff_roundtrip() {
        let (_dir, store) = temp_store();
        let h = Handoff {
            updated_at: "now".into(),
            current_status: "ok".into(),
            blockers: vec!["x".into()],
            next_tasks: vec![],
            risks: vec![],
            files: Default::default(),
            advice: "go".into(),
        };
        store.save_handoff(&h).unwrap();
        let loaded = store.load_handoff().unwrap();
        assert_eq!(loaded.current_status, "ok");
        assert_eq!(loaded.blockers, vec!["x"]);
    }
}
