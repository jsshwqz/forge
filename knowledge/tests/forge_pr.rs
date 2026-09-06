//! V6.0 KNW-101：全环自进化（build_v60c.md 冻结测试，离线可跑）。
//!
//! 冻结测试名：
//! - verified_case_reproduces_failure（只读沙箱拒绝写、类别一致 → reproduced=true）
//! - green_case_rejected_not_reproducible（成功用例 → reproduced=false）
//! - forge_pr_requires_approval_ledger / forge_pr_case_cap_five /
//!   forge_pr_patch_only_touches_knowledge_cases / branch_patch_roundtrip_applies_clean /
//!   main_head_untouched_after_forge_pr（git 类：临时仓库，离线）

use forge_core::ForgeError;
use forge_exec::{EchoTool, ExecutionEngine, PermissionLevel, ToolRouter, WriteFileTool};
use forge_knowledge::forge_pr::{append_approval, forge_pr, load_ledger, Approval};
use forge_knowledge::{case_hash, verify_suggestion, RegressionSuggestion};
use forge_sandbox::{AllowListPolicy, PolicyChain};
use forge_session::InMemorySessionStore;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

fn make_suggestion(pattern: &str, tool: &str, category: &str, input: serde_json::Value) -> RegressionSuggestion {
    RegressionSuggestion {
        pattern: pattern.into(),
        count: 1,
        suggested_case: serde_json::json!({ "tool": tool, "category": category, "input": input }),
    }
}

/// 契约冻结的沙箱口径：只读白名单策略链。
fn sandbox_engine(ws: &Path) -> ExecutionEngine {
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(ws))).unwrap();
    let policy = PolicyChain::new().with(Box::new(AllowListPolicy {
        allowed: vec![PermissionLevel::ReadOnly],
    }));
    ExecutionEngine::new(
        Arc::new(router),
        Arc::new(policy),
        Arc::new(InMemorySessionStore::default()),
        Duration::from_secs(10),
    )
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(cwd).output().unwrap();
    assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_commit(cwd: &Path, msg: &str) {
    git(cwd, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-m", msg]);
}

/// 临时 git 仓库：init + 首提交（提供有效 HEAD）。
fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    git(&root, &["init"]);
    std::fs::write(root.join("README.md"), "test repo").unwrap();
    git(&root, &["add", "README.md"]);
    git_commit(&root, "init");
    (dir, root)
}

fn approval_for(case: &RegressionSuggestion) -> Approval {
    Approval {
        case_hash: case_hash(case),
        pattern: case.pattern.clone(),
        approver: "tester".into(),
        approved_at: chrono::Utc::now(),
    }
}

/// 冻结测试：注入 PermissionDenied 建议；write_file 在只读沙箱被拒、类别一致 → reproduced=true。
#[tokio::test]
async fn verified_case_reproduces_failure() {
    let ws = tempfile::tempdir().unwrap();
    let engine = sandbox_engine(ws.path());
    let s = make_suggestion(
        "PermissionDenied:write_file",
        "write_file",
        "PermissionDenied",
        serde_json::json!({ "path": "x.txt", "content": "hi" }),
    );
    let report = verify_suggestion(&engine, &s).await.unwrap();
    assert!(report.reproduced, "只读沙箱拒绝写必须复现: {:?}", report.detail);
    assert_eq!(report.observed_category.as_deref(), Some("PermissionDenied"));
}

/// 冻结测试：echo 成功用例 → reproduced=false 且 detail 含"未复现"。
#[tokio::test]
async fn green_case_rejected_not_reproducible() {
    let ws = tempfile::tempdir().unwrap();
    let engine = sandbox_engine(ws.path());
    let s = make_suggestion(
        "ToolError:echo",
        "echo",
        "ToolError",
        serde_json::json!({ "goal": "hello" }),
    );
    let report = verify_suggestion(&engine, &s).await.unwrap();
    assert!(!report.reproduced, "成功用例不得判复现");
    assert!(report.detail.contains("未复现"));
    assert_eq!(report.observed_category, None);
}

/// 冻结测试：账本缺该 case_hash → PermissionDenied。
#[tokio::test]
async fn forge_pr_requires_approval_ledger() {
    let (_dir, root) = init_repo();
    let out = tempfile::tempdir().unwrap();
    let s = make_suggestion("PermissionDenied:write_file", "write_file", "PermissionDenied",
        serde_json::json!({ "path": "a.txt", "content": "x" }));
    let err = forge_pr(&root, &[s], &[], out.path()).unwrap_err();
    assert!(matches!(err, ForgeError::PermissionDenied(_)), "实际: {err}");
}

/// 冻结测试：6 条已 approve 用例 → InvalidState（D3 上限 5）。
#[tokio::test]
async fn forge_pr_case_cap_five() {
    let (_dir, root) = init_repo();
    let out = tempfile::tempdir().unwrap();
    let cases: Vec<RegressionSuggestion> = (0..6)
        .map(|i| {
            make_suggestion(
                &format!("PermissionDenied:write_file_{i}"),
                "write_file",
                "PermissionDenied",
                serde_json::json!({ "path": format!("{i}.txt"), "content": "x" }),
            )
        })
        .collect();
    let ledger: Vec<Approval> = cases.iter().map(approval_for).collect();
    let err = forge_pr(&root, &cases, &ledger, out.path()).unwrap_err();
    assert!(matches!(err, ForgeError::InvalidState(_)), "实际: {err}");
}

/// 冻结测试：补丁逐行路径全在白名单内。
#[tokio::test]
async fn forge_pr_patch_only_touches_knowledge_cases() {
    let (_dir, root) = init_repo();
    let out = tempfile::tempdir().unwrap();
    let cases = vec![
        make_suggestion("PermissionDenied:write_file", "write_file", "PermissionDenied",
            serde_json::json!({ "path": "a.txt", "content": "x" })),
        make_suggestion("ToolError:echo", "echo", "ToolError",
            serde_json::json!({ "goal": "boom" })),
    ];
    let ledger: Vec<Approval> = cases.iter().map(approval_for).collect();
    let patch = forge_pr(&root, &cases, &ledger, out.path()).unwrap();
    assert_eq!(patch.case_count, 2);

    let mut patch_files: Vec<PathBuf> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "patch").unwrap_or(false))
        .collect();
    patch_files.sort();
    assert_eq!(patch_files.len(), 2, "每用例一个 commit = 每用例一个补丁");
    for pf in &patch_files {
        let content = std::fs::read_to_string(pf).unwrap();
        for line in content.lines() {
            let path = if let Some(p) = line.strip_prefix("+++ b/") {
                p
            } else if line.starts_with("diff --git a/") {
                line.split(" b/").nth(1).unwrap_or("")
            } else {
                continue;
            };
            assert!(
                path.starts_with("tests/knowledge_cases/") && path.ends_with(".json"),
                "白名单外路径: {path}"
            );
        }
    }
    assert!(patch.patch_path.exists());
}

/// 冻结测试：补丁在另一干净 clone `git apply --check` 通过。
#[tokio::test]
async fn branch_patch_roundtrip_applies_clean() {
    let (_dir, root) = init_repo();
    let out = tempfile::tempdir().unwrap();
    let s = make_suggestion("PermissionDenied:write_file", "write_file", "PermissionDenied",
        serde_json::json!({ "path": "a.txt", "content": "x" }));
    let ledger = vec![approval_for(&s)];
    let patch = forge_pr(&root, &[s], &ledger, out.path()).unwrap();

    let clone_dir = tempfile::tempdir().unwrap();
    let clone = clone_dir.path().join("clone");
    git(clone_dir.path(), &["clone", &root.to_string_lossy(), "clone"]);
    // 干净 clone 上试装补丁（--check 不落盘）
    git(&clone, &["apply", "--check", &patch.patch_path.to_string_lossy()]);
}

/// 冻结测试：生成前后主干 HEAD 一致（R1 红线：永不触主干）。
#[tokio::test]
async fn main_head_untouched_after_forge_pr() {
    let (_dir, root) = init_repo();
    let out = tempfile::tempdir().unwrap();
    let head_before = git(&root, &["rev-parse", "HEAD"]);

    let s = make_suggestion("PermissionDenied:write_file", "write_file", "PermissionDenied",
        serde_json::json!({ "path": "a.txt", "content": "x" }));
    let ledger = vec![approval_for(&s)];
    let patch = forge_pr(&root, &[s], &ledger, out.path()).unwrap();
    assert_eq!(patch.head_before, head_before);

    let head_after = git(&root, &["rev-parse", "HEAD"]);
    assert_eq!(head_before, head_after, "主干 HEAD 不得移动");
}

/// 辅助：账本读写 roundtrip（load_ledger 空文件语义）。
#[tokio::test]
async fn ledger_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("knw_approvals.jsonl");
    assert!(load_ledger(&path).unwrap().is_empty(), "不存在的账本 = 空账本");
    let s = make_suggestion("ToolError:echo", "echo", "ToolError", serde_json::json!({}));
    append_approval(&path, &approval_for(&s)).unwrap();
    let ledger = load_ledger(&path).unwrap();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].case_hash, case_hash(&s));
}
