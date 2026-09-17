//! 工作区内相对路径解析（V8 CTX-001 抽取：Write/Read/List/Edit 四工具共用同一
//! 防逃逸语义，避免四处各写一份而产生漂移）。
//!
//! 冻结三规则（V5.1 WRT-001 沿用，V8 build_v80a.md R1 复述）：
//! 1. 必须为相对路径（拒绝绝对路径 / 盘符 / 前导分隔符）；
//! 2. 不得含 `..` 段（拒绝向上越界）；
//! 3. 拼接改用宿主 `Path::join`，消费方仍受 workspace 根约束。

use forge_core::{ForgeError, ForgeResult};
use std::path::{Path, PathBuf};

/// 把工作区相对路径解析为绝对路径；违反防逃逸规则时返回 [`ForgeError::InvalidState`]。
///
/// `tool` 为调用工具名，仅用于错误文案（排障时可直接定位是哪个工具拒绝了路径）。
/// `rel` 允许为空串（表示工作区根本身，`list_dir` 缺省路径即用此语义）。
pub fn resolve_in_root(root: &Path, rel: &str, tool: &str) -> ForgeResult<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute()
        || rel.contains("..")
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains(':')
    {
        return Err(ForgeError::InvalidState(format!(
            "{tool}: path must be relative inside workspace, got '{rel}'"
        )));
    }
    Ok(root.join(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_paths() {
        let root = Path::new("/ws");
        assert_eq!(resolve_in_root(root, "a.txt", "t").unwrap(), root.join("a.txt"));
        assert_eq!(
            resolve_in_root(root, "sub/dir/b.txt", "t").unwrap(),
            root.join("sub/dir/b.txt")
        );
    }

    #[test]
    fn empty_path_is_workspace_root() {
        let root = Path::new("/ws");
        assert_eq!(resolve_in_root(root, "", "t").unwrap(), root);
    }

    #[test]
    fn rejects_escape_attempts() {
        let root = Path::new("/ws");
        for bad in ["..\\evil.txt", "/abs.txt", "C:\\x.txt", "a/../../b.txt", "\\rooted", ".."] {
            assert!(
                resolve_in_root(root, bad, "t").is_err(),
                "必须拒绝越界路径: {bad}"
            );
        }
    }
}
