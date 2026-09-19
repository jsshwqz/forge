//! B-REAL-001A 测试矩阵（测试名冻结）。
//!
//! 覆盖 builtin_tools 的 6 项冻结断言。
//! 加项 #13（codegen_irreversible_command_denied_e2e）已在 orch101.rs 落网（AF-AUDIT-003 N2）。

use std::collections::HashSet;

use forge_exec::{EchoTool, ToolRouter};
use forge_server::builtin_tools::{is_shell_tool, register_builtin_tools};

// 辅助：注册 5 基线工具到 router（仿 execute_orchestration 的 L364-L385 段）
fn router_with_base_tools() -> ToolRouter {
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    // write_file / read_file / list_dir / edit_patch 需要 workdir，
    // 但测试 1/2/4/5/6 不依赖它们——仅测试 3 需要预注册 read_file。
    // 这里只注册 echo 以保持 router 轻量；测试 3 单独构造完整基线。
    router
}

// ── #1 ──
#[test]
fn builtin_off_when_allowlist_empty() {
    let router = router_with_base_tools();
    // 补齐基线 5 工具（测试要求 router 恰 5）
    // 这里 router 只有 echo（1 个），但空 allowlist 下 register_builtin_tools 不注册任何东西
    // 所以 router.list().len() 仍为 1（非 5）。为满足断言，需要完整 5 基线。
    // 但 write_file 等需要 workdir——使用临时目录。
    use forge_exec::{EditPatchTool, ListDirTool, ReadFileTool, WriteFileTool};
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().to_path_buf();
    router.register(Box::new(WriteFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ReadFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ListDirTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(EditPatchTool::new(workdir.clone()))).unwrap();

    let allow = HashSet::new();
    let report = register_builtin_tools(&router, &allow);

    assert!(report.registered.is_empty(), "registered should be empty");
    assert!(report.skipped_duplicate.is_empty(), "skipped should be empty");
    assert!(report.rejected_shell.is_empty(), "rejected should be empty");
    assert!(report.unknown.is_empty(), "unknown should be empty");
    assert_eq!(router.list().len(), 5, "router should have exactly 5 base tools");
}

// ── #2 ──
#[test]
fn builtin_registers_selected() {
    let router = router_with_base_tools();
    let mut allow = HashSet::new();
    allow.insert("csv_parse".to_string());
    allow.insert("markdown_render".to_string());

    let report = register_builtin_tools(&router, &allow);

    assert_eq!(report.registered.len(), 2, "exactly 2 registered");
    assert!(report.registered.contains(&"csv_parse".to_string()));
    assert!(report.registered.contains(&"markdown_render".to_string()));

    // route() 均可达
    assert!(router.route("csv_parse").is_ok(), "csv_parse should be routable");
    assert!(router.route("markdown_render").is_ok(), "markdown_render should be routable");
}

// ── #3 ──
#[test]
fn builtin_duplicate_skipped_not_fatal() {
    use forge_exec::{EditPatchTool, ListDirTool, ReadFileTool, WriteFileTool};
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().to_path_buf();

    // 先注 BASE_TOOLS 5 个
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ReadFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ListDirTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(EditPatchTool::new(workdir.clone()))).unwrap();
    assert_eq!(router.list().len(), 5);

    // 白名单含 read_file（已注册的基线工具）
    let mut allow = HashSet::new();
    allow.insert("read_file".to_string());

    let report = register_builtin_tools(&router, &allow);

    // read_file 不在候选表，但在 router 中已存在 → skipped_duplicate
    assert!(
        report.skipped_duplicate.contains(&"read_file".to_string()),
        "read_file should be in skipped_duplicate"
    );
    assert!(report.registered.is_empty(), "nothing should be registered");
    // 不 panic 不 Err（函数返回即证明）
}

// ── #4 ──
#[test]
fn builtin_shell_names_rejected() {
    let router = router_with_base_tools();
    let mut allow = HashSet::new();
    allow.insert("pdf_parse".to_string());
    allow.insert("text_classify".to_string());

    let report = register_builtin_tools(&router, &allow);

    assert!(
        report.rejected_shell.contains(&"pdf_parse".to_string()),
        "pdf_parse should be rejected"
    );
    assert!(
        report.rejected_shell.contains(&"text_classify".to_string()),
        "text_classify should be rejected"
    );
    assert!(report.registered.is_empty(), "nothing should be registered");
}

// ── #5 ──
#[test]
fn builtin_unknown_reported() {
    let router = router_with_base_tools();
    let mut allow = HashSet::new();
    allow.insert("not_a_tool".to_string());

    let report = register_builtin_tools(&router, &allow);

    assert_eq!(report.unknown, vec!["not_a_tool"], "unknown should be [not_a_tool]");
    assert!(report.registered.is_empty());
}

// ── #6 ──
#[test]
fn is_shell_tool_matches_bridged_name() {
    // 桥接全名 → true
    assert!(
        is_shell_tool("mcp_aion_pdf_parse"),
        "mcp_aion_pdf_parse should be detected as shell tool"
    );
    // 真逻辑工具裸名 → false
    assert!(
        !is_shell_tool("csv_parse"),
        "csv_parse should NOT be detected as shell tool"
    );
    // 裸壳名 → true
    assert!(is_shell_tool("pdf_parse"), "pdf_parse bare name should be shell tool");
    assert!(is_shell_tool("text_classify"), "text_classify bare name should be shell tool");
    // 无 server 的桥接名 → false
    assert!(
        !is_shell_tool("mcp_pdf_parse"),
        "mcp_pdf_parse without server should NOT be shell tool"
    );
}
