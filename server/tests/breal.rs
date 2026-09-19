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

// ═══════════════════════════════════════════════════════════════════════
// B-REAL-001B 测试矩阵（测试名冻结）#7–#12
// ═══════════════════════════════════════════════════════════════════════

use forge_exec::{EditPatchTool, ListDirTool, PermissionLevel, ReadFileTool, Tool, ToolDescriptor, WriteFileTool};
use forge_plan_llm::{ChatMessage, LlmPlanBackend, LlmPlanner};
use forge_server::planner_view::{
    build_tool_schema_hint, planner_tool_names, SCHEMA_HINT_MAX_BYTES,
};

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// 辅助：完整 5 基线 router（与 #1 同模式）
fn router_with_full_base() -> ToolRouter {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().to_path_buf();
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ReadFileTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(ListDirTool::new(workdir.clone()))).unwrap();
    router.register(Box::new(EditPatchTool::new(workdir.clone()))).unwrap();
    router
}

// 辅助：假桥接工具（mcp_<server>_<tool> 名）
struct FakeBridgedTool {
    descriptor: ToolDescriptor,
}
impl FakeBridgedTool {
    fn new(name: &str) -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: name.into(),
                description: "fake bridged".into(),
                input_schema: json!({"type":"object","properties":{"x":{"type":"string"}}}),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}
#[async_trait]
impl Tool for FakeBridgedTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    async fn invoke(&self, _: Value) -> ForgeResult<Value> {
        Ok(json!({}))
    }
}
use forge_core::ForgeResult;

// 辅助：长描述假工具（用于 #11 预算测试）
struct LongSchemaTool {
    descriptor: ToolDescriptor,
}
impl LongSchemaTool {
    fn new(idx: usize) -> Self {
        let pad = "x".repeat(200);
        Self {
            descriptor: ToolDescriptor {
                name: format!("long_tool_{idx}"),
                description: pad.clone(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "data": {"type": "string", "description": pad}
                    },
                    "required": ["data"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}
#[async_trait]
impl Tool for LongSchemaTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    async fn invoke(&self, _: Value) -> ForgeResult<Value> {
        Ok(json!({}))
    }
}

// 辅助：消息捕获 mock（#12 用）
struct CapturingMock {
    responses: Mutex<Vec<String>>,
    captured: Mutex<Vec<Vec<ChatMessage>>>,
}
#[async_trait]
impl LlmPlanBackend for CapturingMock {
    async fn complete(&self, _model: &str, messages: &[ChatMessage]) -> ForgeResult<String> {
        self.captured.lock().unwrap().push(messages.to_vec());
        let mut g = self.responses.lock().unwrap();
        if g.is_empty() {
            Err(forge_core::ForgeError::InvalidState("mock: exhausted".into()))
        } else {
            Ok(g.remove(0))
        }
    }
}

// ── #7 ──
#[test]
fn planner_tool_names_defaults_to_base_tools() {
    let router = router_with_full_base();
    let names = planner_tool_names(&router);

    assert_eq!(names.len(), 5, "should have exactly 5 tools");
    // 集合相等（BASE_TOOLS = echo/write_file/read_file/list_dir/edit_patch）
    let mut sorted = names.clone();
    sorted.sort();
    let mut expected = vec![
        "echo".to_string(),
        "write_file".to_string(),
        "read_file".to_string(),
        "list_dir".to_string(),
        "edit_patch".to_string(),
    ];
    expected.sort();
    assert_eq!(sorted, expected, "tool set must equal BASE_TOOLS");
}

// ── #8 ──
#[test]
fn planner_tool_names_includes_registered_and_bridged() {
    let router = router_with_full_base();
    // 注入一个真逻辑工具 + 一个假桥接名
    router
        .register(Box::new(FakeBridgedTool::new("mcp_x_y")))
        .unwrap();
    // csv_parse 需要从 parsing crate 注册
    use forge_tools_parsing::CsvParseTool;
    router.register(Box::new(CsvParseTool::new())).unwrap();

    let names = planner_tool_names(&router);

    assert!(names.contains(&"csv_parse".to_string()), "csv_parse must be in list");
    assert!(names.contains(&"mcp_x_y".to_string()), "mcp_x_y must be in list");
    // 整体升序
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "names must be sorted ascending");
}

// ── #9 ──
#[test]
fn planner_tool_names_excludes_shells() {
    let router = router_with_full_base();
    // 注册壳名（通过假工具模拟——真实场景中 is_shell_tool 会拦截）
    router
        .register(Box::new(FakeBridgedTool::new("text_summarize")))
        .unwrap();
    router
        .register(Box::new(FakeBridgedTool::new("mcp_x_pdf_parse")))
        .unwrap();

    let names = planner_tool_names(&router);

    assert!(
        !names.contains(&"text_summarize".to_string()),
        "text_summarize (shell) must be excluded"
    );
    assert!(
        !names.contains(&"mcp_x_pdf_parse".to_string()),
        "mcp_x_pdf_parse (bridged shell) must be excluded"
    );
}

// ── #10 ──
#[test]
fn schema_hint_contains_input_schema() {
    let router = router_with_full_base();
    use forge_tools_parsing::CsvParseTool;
    router.register(Box::new(CsvParseTool::new())).unwrap();

    let names = planner_tool_names(&router);
    let hint = build_tool_schema_hint(&router, &names);

    assert!(
        hint.starts_with("=== Tool input schemas ==="),
        "hint must start with frozen header"
    );
    // csv_parse 的 required 键 "text" 必须出现
    assert!(
        hint.contains("\"text\""),
        "hint must contain csv_parse's required key 'text'"
    );
    assert!(
        hint.contains("csv_parse:"),
        "hint must contain csv_parse entry"
    );
}

// ── #11 ──
#[test]
fn schema_hint_budget_cap() {
    let router = ToolRouter::new();
    // 注册 60 个长描述工具
    for i in 0..60 {
        router
            .register(Box::new(LongSchemaTool::new(i)))
            .unwrap();
    }
    let names = planner_tool_names(&router);
    assert_eq!(names.len(), 60 + 5, "60 long + 5 base (BASE_TOOLS 恒在)");

    let hint = build_tool_schema_hint(&router, &names);

    assert!(
        hint.len() <= SCHEMA_HINT_MAX_BYTES + 32,
        "hint len {} must be <= {} + 32",
        hint.len(),
        SCHEMA_HINT_MAX_BYTES
    );
    assert!(
        hint.contains("(truncated)"),
        "hint must contain (truncated) marker"
    );
}

// ── #12 ──
#[tokio::test]
async fn multistep_prompt_sees_real_tools() {
    let router = router_with_full_base();
    use forge_tools_parsing::CsvParseTool;
    router.register(Box::new(CsvParseTool::new())).unwrap();

    let tools = planner_tool_names(&router);
    let schema_hint = build_tool_schema_hint(&router, &tools);

    // 用捕获 mock 组 LlmPlanner
    let valid_plan = r#"{"steps":[
        {"id":"s1","title":"parse","depends_on":[],"action":{"type":"call","capability":"csv_parse","input":{"text":"a,b\n1,2"}}}
    ]}"#;
    let mock = Arc::new(CapturingMock {
        responses: Mutex::new(vec![valid_plan.to_string()]),
        captured: Mutex::new(vec![]),
    });

    let planner = LlmPlanner {
        backend: mock.clone(),
        model: "test".into(),
        schema_max_attempts: 3,
        tools: tools.clone(),
        ledger: None,
        meter: None,
        brief_mode: false,
        context: Some(schema_hint),
    };

    use forge_planner::Planner;
    use forge_task::Task;
    let task = Task::new(forge_core::TaskId("t1".to_string()), "parse csv".into(), vec![], vec![]);
    let _ = planner.plan(&task).await;

    let captured = mock.captured.lock().unwrap();
    assert!(!captured.is_empty(), "at least one complete call");
    let msgs = &captured[0];

    // system 消息含 "Available capabilities"
    let system_msg = msgs
        .iter()
        .find(|m| m.role == "system")
        .expect("system message must exist");
    assert!(
        system_msg.content.contains("Available capabilities"),
        "system must contain 'Available capabilities'"
    );
    assert!(
        system_msg.content.contains("csv_parse"),
        "system must list csv_parse in capabilities"
    );

    // user 消息含 "csv_parse"（来自 schema hint）
    let user_msg = msgs
        .iter()
        .find(|m| m.role == "user")
        .expect("user message must exist");
    assert!(
        user_msg.content.contains("csv_parse"),
        "user message must contain csv_parse (from schema hint)"
    );
}
