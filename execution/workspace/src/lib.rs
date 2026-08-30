//! forge-workspace：托管工作目录管理器（冻结目录槽位 execution/workspace/）。
//!
//! 为执行/验证提供受管 workdir（`VerificationRequest.workdir` 的来源）：
//! - `create_for(task)`：根目录下创建 `ws-{task}-{短uuid}` 子目录
//! - `cleanup(path)`：删除子目录；路径逃逸防护——canonicalize 后必须仍位于
//!   规范化根目录内，否则拒绝（防 `..` / 符号链接逃逸）；拒绝删除根本身
//! - `list()`：列举现存工作目录（按名排序）
//!
//! 全部同步 API（纯文件系统操作，无异步必要）。

use forge_core::{ForgeError, ForgeResult};
use std::path::{Path, PathBuf};

fn io_err(context: &str, e: std::io::Error) -> ForgeError {
    ForgeError::InvalidState(format!("{context}: {e}"))
}

/// 统一剥离 Windows verbatim 前缀（\\?\），保证比较/返回形式一致。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    match p.as_os_str().to_str() {
        Some(s) if s.starts_with(r"\\?\") => PathBuf::from(&s[4..]),
        _ => p,
    }
}

/// 规范化任意路径（canonicalize + 去 verbatim 前缀）。
fn normalize(path: &Path) -> ForgeResult<PathBuf> {
    Ok(strip_verbatim(
        std::fs::canonicalize(path).map_err(|e| io_err("workspace canonicalize", e))?,
    ))
}

/// 托管工作目录管理器。
pub struct WorkspaceManager {
    /// 规范化后的根目录。
    canon_root: PathBuf,
}

impl WorkspaceManager {
    /// 以 root 为根初始化（不存在则创建；内部保存规范化路径）。
    pub fn new(root: impl Into<PathBuf>) -> ForgeResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| io_err("workspace root", e))?;
        let canon_root = normalize(&root)?;
        Ok(Self { canon_root })
    }

    /// 根目录（规范化）。
    pub fn root(&self) -> &Path {
        &self.canon_root
    }

    /// 为任务创建隔离工作目录，返回其路径。
    pub fn create_for(&self, task_id: &str) -> ForgeResult<PathBuf> {
        let safe: String = task_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if safe.is_empty() {
            return Err(ForgeError::InvalidState(
                "workspace: task_id has no usable characters".into(),
            ));
        }
        // 每次调用唯一：uuid 后缀防撞名（模块契约 ws-{task}-{短uuid}）。
        // 修复前用纯任务 id 命名，同任务连续调用会复用同一目录，
        // 后写的工件静默覆盖先写的——见 create_is_unique_per_call 回归。
        let suffix = uuid::Uuid::new_v4().simple();
        let dir = self.canon_root.join(format!("ws-{}-{}", safe, suffix));
        std::fs::create_dir_all(&dir).map_err(|e| io_err("workspace create", e))?;
        Ok(dir)
    }

    /// 防逃逸校验：path 规范化后必须位于规范化根目录之内。
    fn ensure_inside_root(&self, path: &Path) -> ForgeResult<PathBuf> {
        let canon_path = normalize(path)?;
        if canon_path.starts_with(&self.canon_root) {
            Ok(canon_path)
        } else {
            Err(ForgeError::PermissionDenied(format!(
                "workspace escape denied: {} not under {}",
                canon_path.display(),
                self.canon_root.display()
            )))
        }
    }

    /// 删除托管工作目录（含内容）。逃逸路径与根目录本身一律拒绝。
    pub fn cleanup(&self, path: &Path) -> ForgeResult<()> {
        let inside = self.ensure_inside_root(path)?;
        if inside == self.canon_root {
            return Err(ForgeError::PermissionDenied(
                "workspace: refusing to remove root".into(),
            ));
        }
        std::fs::remove_dir_all(&inside).map_err(|e| io_err("workspace cleanup", e))
    }

    /// 列举现存工作目录（按名称排序）。
    pub fn list(&self) -> ForgeResult<Vec<PathBuf>> {
        let mut out = Vec::new();
        let entries =
            std::fs::read_dir(&self.canon_root).map_err(|e| io_err("workspace list", e))?;
        for e in entries {
            let p = e.map_err(|e| io_err("workspace entry", e))?.path();
            if p.is_dir() {
                out.push(p);
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_cleanup_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();

        let w1 = mgr.create_for("TASK-001").unwrap();
        assert!(w1.starts_with(mgr.root()));
        assert!(w1.exists());
        std::fs::write(w1.join("artifact.txt"), b"x").unwrap();

        assert_eq!(mgr.list().unwrap().len(), 1);

        mgr.cleanup(&w1).unwrap();
        assert!(!w1.exists());
        assert!(mgr.list().unwrap().is_empty());
    }

    #[test]
    fn create_is_unique_per_call() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();
        let a = mgr.create_for("T").unwrap();
        let b = mgr.create_for("T").unwrap();
        assert_ne!(a, b);
        assert_eq!(mgr.list().unwrap().len(), 2);
    }

    /// 高频回归：连续创建同任务目录，必须全部互不相同且都在根下。
    #[test]
    fn create_same_task_many_times_all_unique() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();
        let mut dirs: Vec<PathBuf> = (0..50)
            .map(|_| mgr.create_for("SAME-TASK").unwrap())
            .collect();
        let n = dirs.len();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs.len(), n, "50 次创建应得 50 个不同目录");
        assert_eq!(mgr.list().unwrap().len(), n);
        assert!(dirs.iter().all(|d| d.starts_with(mgr.root())));
        assert!(dirs.iter().all(|d| d.file_name().unwrap().to_str().unwrap().starts_with("ws-SAME-TASK-")));
    }

    /// 清理后新建的目录不复用被删路径。
    #[test]
    fn create_after_cleanup_does_not_reuse_path() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();
        let a = mgr.create_for("T").unwrap();
        mgr.cleanup(&a).unwrap();
        let b = mgr.create_for("T").unwrap();
        assert_ne!(a, b);
        assert!(b.exists());
    }

    #[test]
    fn escape_denied_and_victim_intact() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("precious");
        std::fs::create_dir_all(&victim).unwrap();

        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();

        let evil = root.path().join("..").join(
            outside
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        ).join("precious");
        let err = mgr.cleanup(&evil).unwrap_err();
        assert!(err.to_string().contains("denied"), "got: {err}");
        assert!(victim.exists(), "外部目录必须完好");
    }

    #[test]
    fn refuse_cleanup_root_itself() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();
        let err = mgr.cleanup(root.path()).unwrap_err();
        assert!(err.to_string().contains("refusing to remove root"));
        assert!(root.path().exists());
    }

    #[test]
    fn unusable_task_id_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mgr = WorkspaceManager::new(root.path()).unwrap();
        assert!(mgr.create_for("///###").is_err());
    }
}
