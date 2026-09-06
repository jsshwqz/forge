//! V6.0 KNW-101：人工 approve 账本与本地分支补丁生成（PR 等价物）。
//!
//! 现实约束（build_v60c.md 冻结）：无 CI/无 git 托管平台——"PR" 落地为
//! 本地 git 分支 + commit + format-patch 补丁导出；merge 是人工动作。
//! R1 红线：本模块只做 worktree + commit + format-patch，主干 HEAD 不变。

use crate::suggest::RegressionSuggestion;
use crate::verify::case_hash;
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 固化用例的唯一允许落点（R4 白名单）。
pub const KNOWLEDGE_CASES_DIR: &str = "tests/knowledge_cases";

/// 单补丁用例数上限（D3）。
pub const MAX_CASES_PER_PR: usize = 5;

/// 一条人工批准记录（账本 = artifacts/knw_approvals.jsonl，逐行一个 Approval）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub case_hash: String,
    pub pattern: String,
    pub approver: String,
    pub approved_at: chrono::DateTime<chrono::Utc>,
}

/// 读取账本；文件不存在视为空账本。
pub fn load_ledger(path: &Path) -> ForgeResult<Vec<Approval>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).map_err(|e| {
            ForgeError::InvalidState(format!("approval ledger line {}: {e}", i + 1))
        })?);
    }
    Ok(out)
}

/// 追加一条批准记录。
pub fn append_approval(path: &Path, a: &Approval) -> ForgeResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(a)
        .map_err(|e| ForgeError::InvalidState(format!("approval serialize: {e}")))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// forge_pr 产出。
#[derive(Clone, Debug, Serialize)]
pub struct ForgePatch {
    pub branch: String,
    /// 导出的第一个补丁文件（format-patch 0001-*）。
    pub patch_path: PathBuf,
    pub case_count: usize,
    pub head_before: String,
}

fn git(args: Vec<String>, cwd: &Path) -> ForgeResult<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| ForgeError::InvalidState(format!("git unavailable: {e}")))?;
    if !out.status.success() {
        return Err(ForgeError::InvalidState(format!(
            "git error: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 参数构造：&str / String / PathBuf 混用统一转 String。
macro_rules! sargs {
    ($($a:expr),* $(,)?) => { vec![$($a.to_string()),*] };
}

/// forge_pr 进程级互斥：分支名/工作树名由内容哈希派生，同内容并发会互撞
/// （index.lock / worktree 已存在）——按"提交串行"纪律全局串行化。
static FORGE_PR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// pattern 消毒：小写化后非 [a-z0-9_] → '_'，截断 64 字符（R4）。
fn sanitize_pattern(pattern: &str) -> String {
    let s: String = pattern
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.chars().take(64).collect()
}

/// 生成固化补丁（PR 等价物）。
///
/// D3 硬门禁：每个 case_hash 必须在账本中；用例数 > 5 → InvalidState。
/// 流程：worktree（新分支）→ 白名单路径写用例 → 逐用例 commit →
/// format-patch 导出 → 逐行核对 diff 路径 → 移除 worktree。
/// 不含合并类操作——主干 HEAD 前后不变（R1，冻结测试断言）。
pub fn forge_pr(
    repo_root: &Path,
    cases: &[RegressionSuggestion],
    ledger: &[Approval],
    out_dir: &Path,
) -> ForgeResult<ForgePatch> {
    if cases.is_empty() {
        return Err(ForgeError::InvalidState("forge_pr: no cases".into()));
    }
    if cases.len() > MAX_CASES_PER_PR {
        return Err(ForgeError::InvalidState(format!(
            "forge_pr: D3 cap is {MAX_CASES_PER_PR} cases, got {}",
            cases.len()
        )));
    }
    for c in cases {
        let h = case_hash(c);
        if !ledger.iter().any(|a| a.case_hash == h) {
            return Err(ForgeError::PermissionDenied(format!(
                "case not approved: {h}（先逐条 knowledge-approve）"
            )));
        }
    }

    let head_before = git(sargs!["rev-parse", "HEAD"], repo_root)?;
    let first_hash = case_hash(&cases[0]);
    let date = chrono::Utc::now().format("%Y%m%d");
    let branch = format!("knw_{date}_{}", &first_hash[..8]);
    let wt = std::env::temp_dir().join(format!("knw_wt_{date}_{}", &first_hash[..8]));
    std::fs::create_dir_all(out_dir)?;
    let wt_str = wt.to_string_lossy().to_string();
    // format-patch 以 worktree 为 cwd 运行——out_dir 必须绝对化，否则产物随工作树被删；
    // Windows canonicalize 产生 \\?\ 前缀，git 无法创建该形态目录，剥掉
    let out_canon = std::fs::canonicalize(out_dir).unwrap_or_else(|_| out_dir.to_path_buf());
    let out_str = out_canon
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();

    let _guard = FORGE_PR_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // 残留清理（幂等可重跑）：git 元数据侧 + 孤儿目录侧（旧仓库删除后
    // worktree remove 无从知晓目录，必须直接删除）
    let _ = Command::new("git")
        .args(sargs!["worktree", "remove", "--force", &wt_str])
        .current_dir(repo_root)
        .output();
    let _ = std::fs::remove_dir_all(&wt);
    let _ = git(sargs!["branch", "-D", &branch], repo_root);

    git(sargs!["worktree", "add", &wt_str, "-b", &branch], repo_root)?;

    let result = (|| -> ForgeResult<ForgePatch> {
        for c in cases {
            let rel = format!("{}/{}.json", KNOWLEDGE_CASES_DIR, sanitize_pattern(&c.pattern));
            let abs = wt.join(&rel);
            std::fs::create_dir_all(abs.parent().unwrap())?;
            std::fs::write(
                &abs,
                serde_json::to_string_pretty(&c.suggested_case)
                    .map_err(|e| ForgeError::InvalidState(format!("case serialize: {e}")))?,
            )?;
            git(sargs!["add", &rel], &wt)?;
            git(
                sargs![
                    "-c", "user.name=forge-knw",
                    "-c", "user.email=knw@aion.forge",
                    "commit", "-m", &format!("KNW-101: 固化回归用例 {}", c.pattern),
                ],
                &wt,
            )?;
        }
        git(
            sargs!["format-patch", &format!("{head_before}..HEAD"), "-o", &out_str],
            &wt,
        )?;

        // R4：逐行核对补丁触碰路径，白名单外 → VerificationFailed
        let mut patch_files: Vec<PathBuf> = std::fs::read_dir(out_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "patch").unwrap_or(false))
            .collect();
        patch_files.sort();
        for pf in &patch_files {
            let content = std::fs::read_to_string(pf)?;
            for line in content.lines() {
                // 取补丁触碰的目标路径：优先 "+++ b/<path>"，其次 "diff --git a/x b/y" 的 b/ 侧
                let path = if let Some(p) = line.strip_prefix("+++ b/") {
                    p
                } else if line.starts_with("diff --git a/") {
                    match line.split(" b/").nth(1) {
                        Some(p) => p,
                        None => continue,
                    }
                } else {
                    continue;
                };
                if path.is_empty() {
                    continue;
                }
                let ok = path.starts_with(&format!("{KNOWLEDGE_CASES_DIR}/"))
                    && path.ends_with(".json");
                if !ok {
                    return Err(ForgeError::VerificationFailed(format!(
                        "patch touches non-whitelisted path: {path}"
                    )));
                }
            }
        }
        let patch_path = patch_files
            .first()
            .cloned()
            .ok_or_else(|| ForgeError::InvalidState("forge_pr: no patch produced".into()))?;
        Ok(ForgePatch {
            branch: branch.clone(),
            patch_path,
            case_count: cases.len(),
            head_before: head_before.clone(),
        })
    })();

    match result {
        Ok(patch) => {
            git(sargs!["worktree", "remove", "--force", &wt_str], repo_root)?;
            Ok(patch)
        }
        Err(e) => {
            // 违规清理：worktree 与分支一并移除（R4）
            let _ = git(sargs!["worktree", "remove", "--force", &wt_str], repo_root);
            let _ = git(sargs!["branch", "-D", &branch], repo_root);
            Err(e)
        }
    }
}
