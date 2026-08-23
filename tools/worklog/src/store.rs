//! JSON 存储：PROGRESS / WORKLOG / HANDOFF 的读写。
//!
//! 数据文件默认放在项目根目录（与 AI_WORKFLOW.md 同级）：
//! - progress.json
//! - worklog.json
//! - handoff.json
//!
//! Markdown 视图（PROGRESS.md / WORKLOG.md / HANDOFF.md）由 CLI 从 JSON 导出生成，
//! 避免手工维护两个事实源。
//!
//! ## 并发契约（COMP-003b 加固）
//!
//! 多个 AI 会话可能同时操作同一仓库的状态文件。本层提供三重防护：
//! 1. **原子写**：所有保存先写临时文件再原子重命名，读者不会看到半截 JSON；
//! 2. **跨进程互斥**：CLI 的变更命令通过 Store::lock 在"读取→修改→保存"
//!    全程持有根目录 .worklog.lock 锁（内容为获取时的 UNIX 秒级时间戳，
//!    陈旧锁可被自动接管）；库调用方在做读改写时也必须遵守同一约定；
//! 3. **读时自愈**：load_progress 自动合并重复 task_id 行（按信息保真度
//!    保留最优行），即使历史写入方留下脏数据也会在下次读取时收敛。

use crate::models::{Handoff, ProgressEntry, WorkRecord};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// 根目录锁文件的默认名称。
const LOCK_FILE: &str = ".worklog.lock";
/// 锁获取的默认超时（秒）。
const LOCK_TIMEOUT_SECS: u64 = 10;
/// 超过该年龄（秒）的锁视为陈旧锁，允许接管。
const LOCK_STALE_SECS: u64 = 120;

/// 跨进程文件锁守卫；Drop 时释放。
///
/// 由 Store::lock 创建。持有期间其他进程/命令对同一仓库的变更操作会被阻塞，
/// 从而保证"读取→修改→保存"序列的完整性。
#[derive(Debug)]
pub struct FileLock {
    /// 已持有的锁文件路径；None 表示已释放。
    path: Option<PathBuf>,
}

impl FileLock {
    fn release(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = fs::remove_file(p);
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        self.release();
    }
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

    /// 返回锁文件路径。
    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// 获取跨进程变更锁（默认超时 10 秒）。
    ///
    /// 锁文件内容为获取时刻的 UNIX 秒时间戳；若已存在的锁超过陈旧阈值
    /// 未释放（如进程崩溃残留），会被自动接管。
    ///
    /// 约定：所有"读取→修改→保存"序列必须在持有该守卫期间完成。
    pub fn lock(&self) -> Result<FileLock, StoreError> {
        self.lock_with_timeout(LOCK_TIMEOUT_SECS)
    }

    /// 以指定超时（秒）获取变更锁，供测试与特殊场景使用。
    pub fn lock_with_timeout(&self, timeout_secs: u64) -> Result<FileLock, StoreError> {
        ensure_dir(&self.lock_path())?;
        let path = self.lock_path();
        let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs.max(1));
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    use std::io::Write;
                    writeln!(f, "{ts}")?;
                    return Ok(FileLock { path: Some(path) });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // 检查是否为陈旧锁（内容是获取时刻的 UNIX 秒时间戳）
                    let stale = fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .map(|ts| {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(u64::MAX);
                            now.saturating_sub(ts) > LOCK_STALE_SECS
                        })
                        .unwrap_or(false);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                }
                Err(e) => return Err(e.into()),
            }
            if std::time::Instant::now() >= deadline {
                return Err(StoreError::Invalid(format!(
                    "获取 {} 超时（{} 秒）：可能存在其他会话正在写状态文件",
                    path.display(),
                    timeout_secs
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // ---- PROGRESS ----

    /// 读取任务状态索引；文件不存在时返回空列表。
    ///
    /// 读取时自动合并重复 task_id 行（读时自愈）。
    pub fn load_progress(&self) -> Result<Vec<ProgressEntry>, StoreError> {
        let path = self.progress_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path)?;
        let entries: Vec<ProgressEntry> = serde_json::from_str(&data)?;
        Ok(merge_duplicate_entries(entries))
    }

    /// 保存任务状态索引（原子写）。
    pub fn save_progress(&self, entries: &[ProgressEntry]) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(entries)?;
        atomic_write(&self.progress_path(), json)
    }

    /// 按 task_id 查找进度条目。
    pub fn find_progress(&self, task_id: &str) -> Result<Option<ProgressEntry>, StoreError> {
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

    /// 保存工作日志（原子写）。
    pub fn save_worklog(&self, records: &[WorkRecord]) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(records)?;
        atomic_write(&self.worklog_path(), json)
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
                r.id.strip_prefix(&prefix)
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

    /// 保存交接快照（原子写）。
    pub fn save_handoff(&self, handoff: &Handoff) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(handoff)?;
        atomic_write(&self.handoff_path(), json)
    }
}

/// 原子写：先写同目录临时文件，再重命名覆盖目标。
///
/// 重命名在同一目录内进行（同一卷），POSIX 与 Windows 上均为原子替换语义；
/// 任何中途失败都不会破坏既有文件。
fn atomic_write(path: &Path, content: String) -> Result<(), StoreError> {
    ensure_dir(path)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// 单行的信息保真度评分：last_record 权重最高，其次有效 commit 与 owner。
///
/// 用于重复行合并时挑选应保留的规范行。以 n/a 开头的 commit 视为无有效值。
fn entry_fidelity(e: &ProgressEntry) -> u32 {
    let mut score = 0;
    if e.last_record.is_some() {
        score += 2;
    }
    if e.commit
        .as_deref()
        .map(|c| !c.is_empty() && !c.starts_with("n/a"))
        .unwrap_or(false)
    {
        score += 1;
    }
    if e.owner.is_some() {
        score += 1;
    }
    score
}

/// 合并重复 task_id 行：每个 ID 仅保留保真度最高的一行（平分取靠后者，
/// 因为闭环行总是后写入），其余丢弃；保留各 ID 首次出现的相对顺序。
pub fn merge_duplicate_entries(entries: Vec<ProgressEntry>) -> Vec<ProgressEntry> {
    use std::collections::HashMap;
    // 先为每个 id 选出规范行索引
    let mut best: HashMap<&str, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        match best.get(e.task_id.as_str()) {
            Some(&j) if entry_fidelity(&entries[j]) > entry_fidelity(e) => {}
            _ => {
                best.insert(e.task_id.as_str(), i);
            }
        }
    }
    let mut seen: HashMap<&str, ()> = HashMap::new();
    let mut out = Vec::with_capacity(best.len());
    for (i, e) in entries.iter().enumerate() {
        if seen.contains_key(e.task_id.as_str()) {
            continue;
        }
        if best.get(e.task_id.as_str()) == Some(&i) {
            seen.insert(e.task_id.as_str(), ());
            out.push(e.clone());
        }
    }
    out
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

    fn entry(id: &str, name: &str, status: TaskStatus) -> ProgressEntry {
        ProgressEntry {
            task_id: id.into(),
            name: name.into(),
            status,
            owner: None,
            last_record: None,
            commit: None,
        }
    }

    #[test]
    fn test_progress_roundtrip() {
        let (_dir, store) = temp_store();
        let entries = vec![entry("TASK-001", "测试", TaskStatus::Wip)];
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
            .append_record(
                RecordKind::R1Completed,
                "2026-08-22",
                Some("TASK-001".into()),
                "a",
                "body",
            )
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

    // ---- COMP-003b 并发加固 ----

    #[test]
    fn test_load_progress_dedupes_and_keeps_best_row() {
        let (dir, store) = temp_store();
        // 手工写入带重复 task_id 的原始 JSON（弱行在前、强行在后，字段顺序不同）
        let raw = r#"[
          {
            "last_record": null,
            "name": "server 接入 PG 持久化(env 驱动)",
            "commit": "ffd86a8",
            "task_id": "INT-001",
            "status": "Completed",
            "owner": null
          },
          {
            "task_id": "INT-001",
            "name": "server 接入 PG 持久化",
            "status": "Completed",
            "owner": "builder-a",
            "last_record": "R1-012",
            "commit": "ffd86a8"
          },
          {
            "task_id": "SOLO-001",
            "name": "无重复任务",
            "status": "NotStarted",
            "owner": null,
            "last_record": null,
            "commit": null
          }
        ]"#;
        fs::write(store.progress_path(), raw).unwrap();
        let loaded = store.load_progress().unwrap();
        assert_eq!(loaded.len(), 2, "重复行应在读取时合并");
        assert_eq!(loaded[0].task_id, "INT-001");
        assert_eq!(
            loaded[0].name, "server 接入 PG 持久化",
            "应保留保真度更高的行（含 last_record/owner）"
        );
        assert_eq!(loaded[0].last_record.as_deref(), Some("R1-012"));
        assert_eq!(loaded[1].task_id, "SOLO-001");
        let _ = dir.keep();
    }

    #[test]
    fn test_save_leaves_no_tmp_residue() {
        let (dir, store) = temp_store();
        let entries = vec![entry("TASK-001", "x", TaskStatus::NotStarted)];
        store.save_progress(&entries).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "json.tmp")
                    .unwrap_or(false)
            })
            .collect();
        assert!(leftovers.is_empty(), "不应残留 .json.tmp 文件");
        assert_eq!(store.load_progress().unwrap().len(), 1);
    }

    #[test]
    fn test_lock_acquire_release_reacquire() {
        let (_dir, store) = temp_store();
        {
            let _g = store.lock_with_timeout(1).unwrap();
        }
        let _g2 = store.lock_with_timeout(1).unwrap(); // 释放后可重新获取
    }

    #[test]
    fn test_lock_conflict_times_out() {
        let (_dir, store) = temp_store();
        let _g = store.lock_with_timeout(1).unwrap();
        let err = store.lock_with_timeout(1).unwrap_err();
        assert!(err.to_string().contains("超时"), "应为超时错误: {err}");
    }

    #[test]
    fn test_lock_stale_takeover() {
        let (dir, store) = temp_store();
        // 手工放置一个 1 小时前的陈旧锁
        let old_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 3600;
        fs::write(
            store.lock_path(),
            format!(
                "{old_ts}
"
            ),
        )
        .unwrap();
        let _g = store.lock_with_timeout(1).unwrap(); // 应接管而非超时
        assert!(store.lock_path().exists());
        let _ = dir.keep();
    }
}
