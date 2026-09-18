//! 任务工作区 Git 集成（V8.0 GIT-001，D11：每任务一个 commit）。
//!
//! `git_enabled()` 由 `FORGE_GIT_TASK=1` 显式开启，缺省关闭（零回归）。
//! `commit_task_workdir()` 在任务结束时对工作区做 git init（若未 init）、单 commit、
//! 以及 `format-patch` 补丁导出（供人工审阅）。红线沿用 KNW-101：工作区是独立
//! repo，不触主干，合入是人工动作。

use forge_core::{ForgeError, ForgeResult};
use std::path::{Path, PathBuf};

/// Git 集成是否启用：`FORGE_GIT_TASK=1` 显式开启，缺省关闭。
pub fn git_enabled() -> bool {
    std::env::var("FORGE_GIT_TASK").ok().as_deref() == Some("1")
}

fn git(args: &[&str], cwd: &Path) -> ForgeResult<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| ForgeError::InvalidState(format!("task_git: git unavailable: {e}")))?;
    if !out.status.success() {
        return Err(ForgeError::InvalidState(format!(
            "task_git: git error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 任务工作区收尾：git init（若未 init）+ 单 commit（D11）+ 补丁导出。
/// 返回导出的首个补丁路径（无补丁时为 None）。
pub fn commit_task_workdir(workdir: &Path, task_id: &str) -> ForgeResult<Option<PathBuf>> {
    // 1. git init（未 init 时）+ 独立 user config（不依赖全局配置，避免 commit 失败）。
    if !workdir.join(".git").exists() {
        git(&["init", "-q"], workdir)?;
        git(&["config", "user.name", "forge"], workdir)?;
        git(&["config", "user.email", "forge@local"], workdir)?;
    }

    // 2. 每任务一个 commit（D11），--allow-empty 保证空产物也留痕。
    git(&["add", "-A"], workdir)?;
    let msg = format!("task {task_id} completion");
    git(&["commit", "-q", "--allow-empty", "-m", &msg], workdir)?;

    // 3. 补丁导出（供人工审阅；红线：不触主干，合入是人工动作）。
    let out_dir = workdir
        .parent()
        .unwrap_or(workdir)
        .join(format!("{task_id}-patches"));
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| ForgeError::InvalidState(format!("task_git: mkdir patches: {e}")))?;
    let out_str = out_dir.to_string_lossy().to_string();
    git(&["format-patch", "--root", "-1", "-o", &out_str], workdir)?;

    let mut patches: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map_err(|e| ForgeError::InvalidState(format!("task_git: list patches: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "patch").unwrap_or(false))
        .collect();
    patches.sort();
    Ok(patches.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_disabled_by_default() {
        // 默认未设 FORGE_GIT_TASK=1 → 关闭。
        std::env::remove_var("FORGE_GIT_TASK");
        assert!(!git_enabled());
    }

    #[test]
    fn commit_task_workdir_produces_commit_and_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("ws-T");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(workdir.join("out.txt"), "hello").unwrap();

        let patch = commit_task_workdir(&workdir, "T").unwrap();
        // 补丁已导出
        assert!(patch.is_some(), "应导出一个补丁");
        assert!(patch.unwrap().exists());
        // git 已 init
        assert!(workdir.join(".git").exists());
        // 提交记录存在
        let log = git(&["log", "--oneline"], &workdir).unwrap();
        assert!(log.contains("task T completion"), "commit 信息应含任务 ID: {log}");
    }
}