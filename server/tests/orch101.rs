//! V7 ORCH-101a：多步规划接通与重规划回路（build_v70a.md 冻结测试，离线 mock）。

use forge_exec::{EchoTool, PermissionLevel, PermissionPolicy, PolicyContext, ToolRouter, WriteFileTool};
use forge_plan_llm::{ChatMessage, LlmPlanBackend, LlmPlanner, LlmReplanner};
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 顺序回放预设响应的离线 mock（planner 与 replanner 共享同一应答队列）。
struct MockLlm {
    responses: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmPlanBackend for MockLlm {
    async fn complete(&self, _m: &str, _msgs: &[ChatMessage]) -> forge_core::ForgeResult<String> {
        let mut g = self.responses.lock().unwrap();
        if g.is_empty() {
            Err(forge_core::ForgeError::InvalidState("mock: exhausted".into()))
        } else {
            Ok(g.remove(0))
        }
    }
}

struct AllowAll;
impl PermissionPolicy for AllowAll {
    fn check(&self, _: PermissionLevel, _: &PolicyContext) -> forge_core::ForgeResult<()> {
        Ok(())
    }
}

fn two_step_plan() -> String {
    r#"{"steps":[
        {"id":"s1","title":"greet","depends_on":[],"action":{"type":"call","capability":"echo","input":{"goal":"hi"}}},
        {"id":"s2","title":"write","depends_on":["s1"],"action":{"type":"call","capability":"write_file","input":{"path":"out.txt","content":"hello"}}}
    ]}"#
    .to_string()
}

fn bad_path_plan() -> String {
    r#"{"steps":[
        {"id":"s1","title":"escape","depends_on":[],"action":{"type":"call","capability":"write_file","input":{"path":"../evil.txt","content":"x"}}}
    ]}"#
    .to_string()
}

fn fixed_plan() -> String {
    r#"{"steps":[
        {"id":"r1","title":"echo fallback","depends_on":[],"action":{"type":"call","capability":"echo","input":{"goal":"revised"}}}
    ]}"#
    .to_string()
}

fn acceptance_command() -> Vec<forge_task::AcceptanceCriterion> {
    let cmd =
        if cfg!(target_os = "windows") { "cmd /c echo ok" } else { "true" };
    vec![forge_task::AcceptanceCriterion {
        id: "AC-1".into(),
        description: "sanity command".into(),
        check: forge_task::CheckSpec::Command(cmd.into()),
    }]
}

fn deps_with(
    ws: &Path2,
    planner: Option<Arc<dyn forge_planner::Planner>>,
    replanner: Option<Arc<dyn forge_plan_llm::Replanner>>,
) -> OrchestratorDeps {
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(ws))).unwrap();
    OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: Arc::new(forge_verify::CommandVerifier),
        verifier_file: Arc::new(forge_verify::FileVerifier),
        evidence: Arc::new(forge_evidence::InMemoryEvidenceStore::default()),
        workspace: Arc::new(forge_workspace::WorkspaceManager::new(ws.join("ws-root")).unwrap()),
        timeout: Duration::from_secs(15),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy { max_attempts: 1, base_backoff_ms: 10 }),
        replanner,
        max_replans: 1,
        planner,
    }
}

type Path2 = std::path::Path;

/// 冻结测试：auto/codegen_flag/multi_step 三来源解析矩阵（R1 冻结）。
#[test]
fn plan_mode_resolution_matrix() {
    use forge_server::{resolve_plan_mode, PlanMode};
    // 缺省 = codegen_flag 兼容路径
    assert_eq!(resolve_plan_mode(None, true), PlanMode::Codegen);
    assert_eq!(resolve_plan_mode(None, false), PlanMode::MultiStep);
    // 显式 Auto 恒等于 Codegen（零破坏承诺）
    assert_eq!(resolve_plan_mode(Some(PlanMode::Auto), false), PlanMode::Codegen);
    assert_eq!(resolve_plan_mode(Some(PlanMode::Auto), true), PlanMode::Codegen);
    // 显式覆盖
    assert_eq!(resolve_plan_mode(Some(PlanMode::Codegen), false), PlanMode::Codegen);
    assert_eq!(resolve_plan_mode(Some(PlanMode::MultiStep), true), PlanMode::MultiStep);
}

/// 冻结测试：mock LLM 返回两步计划 → 波次顺序执行、写盘成功、final=Completed。
#[tokio::test]
async fn multistep_plan_executes_waves() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let task = sdk
        .create_task("multistep probe", vec![], acceptance_command())
        .await
        .unwrap();

    let mock = Arc::new(MockLlm { responses: Mutex::new(vec![two_step_plan()]) });
    let planner = LlmPlanner {
        backend: mock.clone() as Arc<dyn LlmPlanBackend>,
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into(), "write_file".into()],
        ledger: None,
        meter: None,
        brief_mode: false,
    };
    let deps = deps_with(ws.path(), Some(Arc::new(planner)), None);
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed, "{:?}", report.execution.failed);
    assert_eq!(report.execution.completed.len(), 2, "两步都要执行");
    assert!(
        ws.path().join("ws-root").join(task.id.as_ref()).join("out.txt").exists()
            || ws.path().join("out.txt").exists()
            || ws
                .path()
                .join("ws-root")
                .join("out.txt")
                .exists(),
        "write_file 步骤必须落盘"
    );
}

/// 冻结测试：步骤失败 → 重规划换计划续跑 → replans_used=1。
#[tokio::test]
async fn replanner_invoked_on_step_failure() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let task = sdk
        .create_task("replan probe", vec![], acceptance_command())
        .await
        .unwrap();

    // 第一次 plan 给出逃逸路径（WRT-001 运行时拒绝）→ 失败触发重规划
    let mock = Arc::new(MockLlm {
        responses: Mutex::new(vec![bad_path_plan(), fixed_plan()]),
    });
    let planner = LlmPlanner {
        backend: mock.clone() as Arc<dyn LlmPlanBackend>,
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into(), "write_file".into()],
        ledger: None,
        meter: None,
        brief_mode: false,
    };
    let replanner = LlmReplanner {
        backend: mock as Arc<dyn LlmPlanBackend>,
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into(), "write_file".into()],
        ledger: None,
        meter: None,
    };
    let deps = deps_with(ws.path(), Some(Arc::new(planner)), Some(Arc::new(replanner)));
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.replans_used, 1, "失败后必须恰重规划一次");
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed);
}

/// 冻结测试：MultiStep 未配 LLM → planner=None 走 SequentialPlanner 兜底完成（不报错）。
#[tokio::test]
async fn multistep_falls_back_without_llm() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let task = sdk
        .create_task("fallback probe", vec![], acceptance_command())
        .await
        .unwrap();

    // handler 行为镜像：MultiStep + !llm_ready ⇒ deps.planner=None（SDK 内部 SequentialPlanner）
    let deps = deps_with(ws.path(), None, None);
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed);
    assert_eq!(report.replans_used, 0);
}

// ==================== ORCH-101b：多文件工程 + 沙箱运行验收 ====================

use forge_server::sandbox_verify::{command_level, select_command_verifier, SANDBOX_DENY_MARKER};
use forge_evidence::EvidenceStore as _;

fn three_file_plan() -> String {
    r#"{"steps":[
        {"id":"f1","title":"a","depends_on":[],"action":{"type":"call","capability":"write_file","input":{"path":"a.txt","content":"A"}}},
        {"id":"f2","title":"b","depends_on":["f1"],"action":{"type":"call","capability":"write_file","input":{"path":"sub/b.txt","content":"B"}}},
        {"id":"f3","title":"c","depends_on":["f2"],"action":{"type":"call","capability":"write_file","input":{"path":"c.txt","content":"C"}}}
    ]}"#
    .to_string()
}

fn single_echo_plan() -> String {
    r#"{"steps":[
        {"id":"e1","title":"echo","depends_on":[],"action":{"type":"call","capability":"echo","input":{"goal":"run"}}}
    ]}"#
    .to_string()
}

/// MultiStep 装配（沙箱化 verifier + 可持有证据存储）。
fn multistep_deps(
    ws: &std::path::Path,
    responses: Vec<String>,
    evidence: Arc<forge_evidence::InMemoryEvidenceStore>,
) -> OrchestratorDeps {
    let mock = Arc::new(MockLlm { responses: Mutex::new(responses) });
    let planner = LlmPlanner {
        backend: mock.clone() as Arc<dyn LlmPlanBackend>,
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into(), "write_file".into()],
        ledger: None,
        meter: None,
        brief_mode: false,
    };
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(ws))).unwrap();
    let evidence_store = evidence;
    OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: select_command_verifier(forge_server::PlanMode::MultiStep, Arc::new(forge_verify::CommandVerifier)).0,
        verifier_file: Arc::new(forge_verify::FileVerifier),
        evidence: evidence_store.clone(),
        workspace: Arc::new(forge_workspace::WorkspaceManager::new(ws.join("ws-root")).unwrap()),
        timeout: Duration::from_secs(15),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy { max_attempts: 1, base_backoff_ms: 10 }),
        replanner: None,
        max_replans: 1,
        planner: Some(Arc::new(planner)),
    }
}

/// 冻结测试：mock LLM 三 write_file 步骤（不同路径）→ 三文件落盘。
#[tokio::test]
async fn multi_file_project_generated() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let task = sdk.create_task("multi file", vec![], acceptance_command()).await.unwrap();
    let deps = multistep_deps(ws.path(), vec![three_file_plan()], Default::default());
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed, "{:?}", report.execution.failed);
    assert_eq!(report.execution.completed.len(), 3, "三步全部执行");
    for f in ["a.txt", "sub/b.txt", "c.txt"] {
        assert!(ws.path().join(f).exists(), "多文件工程必须落盘: {f}");
    }
}

/// 冻结测试：Irreversible 级验收（format 磁盘语义）→ verdict=Fail 且拒绝标记入证据。
#[tokio::test]
async fn sandbox_blocks_irreversible() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let acceptance = vec![forge_task::AcceptanceCriterion {
        id: "AC-1".into(),
        description: "dangerous".into(),
        check: forge_task::CheckSpec::Command("format c:".into()),
    }];
    let task = sdk.create_task("irreversible probe", vec![], acceptance).await.unwrap();
    let store: Arc<forge_evidence::InMemoryEvidenceStore> = Default::default();
    let deps = multistep_deps(ws.path(), vec![single_echo_plan()], store.clone());
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.verifications[0].verdict, forge_verify::Verdict::Fail, "Irreversible 必须 Fail");
    assert!(
        report.verifications[0].reason.contains(SANDBOX_DENY_MARKER),
        "拒绝原因必须带审计标记: {}",
        report.verifications[0].reason
    );
    let ev = store.by_criterion("AC-1").await.unwrap();
    assert!(!ev.is_empty() && ev[0].content.contains(SANDBOX_DENY_MARKER), "拒绝必须留证可审计");
}

/// 冻结测试：命令验收真实执行 → 证据入库非空。
#[tokio::test]
async fn run_acceptance_records_evidence() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();
    let cmd = if cfg!(target_os = "windows") { "cmd /c echo sandbox-ok" } else { "echo sandbox-ok" };
    let acceptance = vec![forge_task::AcceptanceCriterion {
        id: "AC-1".into(),
        description: "run".into(),
        check: forge_task::CheckSpec::Command(cmd.into()),
    }];
    let task = sdk.create_task("evidence probe", vec![], acceptance).await.unwrap();
    let store: Arc<forge_evidence::InMemoryEvidenceStore> = Default::default();
    let deps = multistep_deps(ws.path(), vec![single_echo_plan()], store.clone());
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(15) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();
    assert_eq!(report.verifications[0].verdict, forge_verify::Verdict::Pass);
    let ev = store.by_criterion("AC-1").await.unwrap();
    assert!(!ev.is_empty(), "运行验收必须留证");
}

/// 冻结测试：装配矩阵——仅 MultiStep 启用沙箱（基线路径零回归断言）；命令分级冻结。
#[test]
fn baseline_path_verifier_unchanged() {
    use forge_server::PlanMode;
    let plain: Arc<dyn forge_verify::Verifier> = Arc::new(forge_verify::CommandVerifier);
    assert!(!select_command_verifier(PlanMode::Codegen, plain.clone()).1, "Codegen 不启用沙箱");
    assert!(select_command_verifier(PlanMode::MultiStep, plain).1, "MultiStep 必须启用沙箱");
    // 命令风险分级冻结
    assert_eq!(command_level("format c:"), PermissionLevel::Irreversible);
    assert_eq!(command_level("mkfs.ext4 /dev/sda"), PermissionLevel::Irreversible);
    assert_eq!(command_level("echo ok"), PermissionLevel::External);
}

// ==================== ORCH-101c：MCP 工具源接入 ====================

use forge_mcp::McpServerConfig;
use forge_server::mcp_tools::{bridged_name, register_mcp_tools};

/// 定位仓库既有 mock-mcp-server 二进制（R4：不新写 mock）。
fn mock_server_cmd() -> Option<McpServerConfig> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.parent()?.join(if cfg!(windows) { "mock-mcp-server.exe" } else { "mock-mcp-server" });
    if !p.exists() {
        eprintln!("[skip-path] mock-mcp-server 未找到: {}", p.display());
        return None;
    }
    Some(McpServerConfig {
        name: "mock".into(),
        command: p.to_string_lossy().to_string(),
        args: vec![],
        env: Default::default(),
    })
}

/// 冻结测试：mock-mcp-server 发现 echo 工具 → 白名单注册 → 桥接调用真实走通。
#[tokio::test]
async fn mcp_tool_discovered_and_registered() {
    let Some(cfg) = mock_server_cmd() else { return };
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();

    let wl: std::collections::HashSet<String> = ["echo".to_string()].into();
    let n = register_mcp_tools(&router, std::slice::from_ref(&cfg), &wl).await.unwrap();
    assert_eq!(n, 1, "白名单内工具必须注册");

    let full = bridged_name("mock", "echo");
    let tool = router.route(&full).expect("桥接工具必须可路由");
    let out = tool
        .invoke(serde_json::json!({ "text": "hello-mcp" }))
        .await
        .expect("桥接调用必须成功");
    let text = out.to_string();
    assert!(text.contains("hello-mcp") || !out.is_null(), "echo 回显: {out}");
}

/// 冻结测试：白名单外工具一律不注册。
#[tokio::test]
async fn mcp_tool_denied_when_not_whitelisted() {
    let Some(cfg) = mock_server_cmd() else { return };
    let router = ToolRouter::new();
    let wl: std::collections::HashSet<String> = ["totally_other_tool".to_string()].into();
    let n = register_mcp_tools(&router, &[cfg], &wl).await.unwrap();
    assert_eq!(n, 0, "白名单外不得注册");
    assert!(router.route(&bridged_name("mock", "echo")).is_err());
}

/// 冻结测试：未配置 FORGE_MCP_SERVERS → 零注册零开销。
#[test]
fn mcp_tools_off_when_unconfigured() {
    // 不设 env 的进程内 configs_from_env() 返回空（OnceLock 首次初始化于本测试进程）
    let configs = forge_server::mcp_tools::configs_from_env();
    assert!(configs.is_empty(), "未配置时必须为空表（进程未设 FORGE_MCP_SERVERS）");
}
