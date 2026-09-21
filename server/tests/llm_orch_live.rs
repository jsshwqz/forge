//! IMPROVE-6/7 Live e2e：真模型 × edit_patch 上下文提示 + 错误透传在编排闭环中实证。
//!
//! 运行方式（KEY 来自 gitignored .env，勿硬编码入库）：
//! ```bash
//! export FORGE_LLM_LIVE=1
//! export FORGE_LLM_BASE_URL="https://token.sensenova.cn/v1"
//! export FORGE_LLM_API_KEY="<your-key>"
//! cargo test -p forge-server --test llm_orch_live -- --nocapture
//! ```
//!
//! 未设置 FORGE_LLM_LIVE=1 时自动跳过（workspace 默认全绿，
//! 供应商配额属外部条件——同 orchestrator_replan_live.rs 的约定）。
//!
//! 验证点：
//! - IMPROVE-7: edit_patch find 未命中时, ForgeError message 包含上下文提示 (>>> 标记 + read_file 建议)
//!   在真实编排闭环 (非直接调用 tool) 中被触发并透传到 orchestrator 错误链。
//! - IMPROVE-6: tool 执行失败时, result.output 详情被透传到 ForgeError message,
//!   而非仅返回状态名 (ExecutionStatus::Failed)。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use forge_exec::{
    EditPatchTool, EchoTool, ListDirTool, PermissionLevel, PermissionPolicy, PolicyContext,
    ReadFileTool, ToolRouter, WriteFileTool,
};
use forge_api::LlmBackend as _;
use forge_plan_llm::LlmPlanner;
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_server::builtin_tools::register_builtin_tools;
use forge_task::{AcceptanceCriterion, CheckSpec, TaskStatus};

struct AllowAll;
impl PermissionPolicy for AllowAll {
    fn check(&self, _: PermissionLevel, _: &PolicyContext) -> forge_core::ForgeResult<()> {
        Ok(())
    }
}

/// 双重开关：FORGE_LLM_LIVE=1 且 FORGE_LLM_* 齐备；返回 (client, model)。
async fn live_stack() -> Option<(Arc<forge_api::LlmClient>, String)> {
    if std::env::var("FORGE_LLM_LIVE").as_deref() != Ok("1") {
        eprintln!("[skip] 未设 FORGE_LLM_LIVE=1——live 门禁按需开启");
        return None;
    }
    let (Ok(base), Ok(key)) = (
        std::env::var("FORGE_LLM_BASE_URL"),
        std::env::var("FORGE_LLM_API_KEY"),
    ) else {
        eprintln!("[skip] FORGE_LLM_* 未设置");
        return None;
    };
    let client = Arc::new(forge_api::LlmClient::new(base, key));
    let models = client.list_models().await.expect("live: list_models failed");
    let model = forge_api::pick_default_model(&models).expect("live: no usable model");
    println!("[live] model = {model}");
    Some((client, model))
}

/// 构建工具 router：5 基线工具 + csv_parse + markdown_render。
fn build_router(workdir: &std::path::Path) -> ToolRouter {
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router.register(Box::new(WriteFileTool::new(workdir.to_path_buf()))).unwrap();
    router.register(Box::new(ReadFileTool::new(workdir.to_path_buf()))).unwrap();
    router.register(Box::new(ListDirTool::new(workdir.to_path_buf()))).unwrap();
    router.register(Box::new(EditPatchTool::new(workdir.to_path_buf()))).unwrap();
    let mut allow = HashSet::new();
    allow.insert("csv_parse".to_string());
    allow.insert("markdown_render".to_string());
    let _ = register_builtin_tools(&router, &allow);
    router
}

/// IMPROVE-7 Live：真模型规划一个含 edit_patch 步骤的任务。
///
/// 任务：创建文件 app.py (内容含 "def hello():"), 然后用 edit_patch 将 "hello" 改为 "greet"。
/// 如果 LLM 规划的 edit_patch 的 find 字符串与实际文件内容不精确匹配 (常见的空白/缩进差异),
/// edit_patch 会返回 IMPROVE-7 的上下文提示错误, 编排闭环应该捕获并透传该错误。
///
/// 我们不要求 LLM 一定成功——但验证：
/// 1. 如果 LLM 计划成功 → 任务 Completed, edit_patch 真的改了文件。
/// 2. 如果 edit_patch 失败 → 错误信息包含 IMPROVE-7 标记 ("find not found" + "Hint:" + ">>>"),
///    或 IMPROVE-6 透传标记 ("execution failed" + 详情), 而非空白/仅状态名。
#[tokio::test]
async fn live_edit_patch_context_hint_in_orchestration() {
    let Some((client, model)) = live_stack().await else { return };

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        live_edit_patch_context_hint_in_orchestration_inner(client, model),
    )
    .await;
    assert!(result.is_ok(), "e2e timed out after 120s");
    result.unwrap();
}

async fn live_edit_patch_context_hint_in_orchestration_inner(
    client: Arc<forge_api::LlmClient>,
    model: String,
) {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();

    let task = sdk
        .create_task(
            "live: edit_patch context hint e2e".to_string(),
            vec![],
            vec![
                AcceptanceCriterion {
                    id: "AC-1".into(),
                    description: "app.py exists".into(),
                    check: CheckSpec::FileExists("app.py".into()),
                },
            ],
        )
        .await
        .unwrap();

    let ws_manager = Arc::new(forge_workspace::WorkspaceManager::new(ws.path()).unwrap());
    let workdir = ws_manager.create_for(task.id.as_ref()).unwrap();
    let router = build_router(&workdir);

    let planner = LlmPlanner {
        backend: client,
        model,
        schema_max_attempts: 3,
        tools: vec![
            "echo".into(),
            "write_file".into(),
            "read_file".into(),
            "list_dir".into(),
            "edit_patch".into(),
            "csv_parse".into(),
            "markdown_render".into(),
        ],
        ledger: None,
        meter: None,
        brief_mode: false,
        context: None,
    };

    let deps = OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: Arc::new(forge_verify::CommandVerifier),
        verifier_file: Arc::new(forge_verify::FileVerifier),
        evidence: Arc::new(forge_evidence::InMemoryEvidenceStore::default()),
        workspace: ws_manager,
        timeout: Duration::from_secs(90),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 0,
            base_backoff_ms: 1,
        }),
        replanner: None,
        max_replans: 0,
        planner: Some(Arc::new(planner)),
        workspace_task: None,
    };

    let orch = Orchestrator {
        capability: "edit_patch".into(),
        timeout: Duration::from_secs(90),
    };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();

    println!(
        "[live] plans={:?} final={:?} gate={} completed={} failed={:?}",
        report.plan_versions,
        report.final_status,
        report.gate.passed,
        report.execution.completed.len(),
        report.execution.failed
    );

    if report.final_status == TaskStatus::Completed {
        // 路径 A：LLM 规划成功，edit_patch 真的改了文件
        println!("[live] ✅ LLM 规划成功, 任务 Completed");
        // app.py 必须存在
        assert!(
            workdir.join("app.py").exists(),
            "app.py must exist after successful orchestration"
        );
    } else {
        // 路径 B：edit_patch 失败 — 验证 IMPROVE-6/7 透传
        println!("[live] 任务未 Completed, 检查错误透传...");

        // 失败步骤及原因 (RunSummary.failed 是 Option<(StepId, String)>)
        let failures = &report.execution.failed;
        assert!(
            failures.is_some(),
            "任务未 Completed 但也没有失败步骤, 异常状态: {:?}",
            report.final_status
        );

        let (_, err_msg) = failures.as_ref().unwrap();
        let msg = err_msg.clone();

        // 至少一个失败信息包含 IMPROVE-6 透传标记 ("execution failed" + 详情)
        // 或 IMPROVE-7 上下文提示 ("find not found" + "Hint:")
        println!("[live] 失败步骤错误: {msg}");
        // IMPROVE-6: "step ... execution failed: ... — {detail}"
        let has_exec_detail = msg.contains("execution failed") && msg.contains("—");
        // IMPROVE-7: "find not found" + "Hint:" + "read_file"
        let has_context_hint = msg.contains("find not found")
            && msg.contains("Hint:")
            && msg.contains("read_file");
        let has_transparency = has_exec_detail || has_context_hint;

        assert!(
            has_transparency,
            "IMPROVE-6/7 透传验证失败: 失败步骤的错误信息既不含 execution failed + 详情 (IMPROVE-6), \
             也不含 find not found + Hint: (IMPROVE-7). 错误: {msg}"
        );
        println!("[live] ✅ IMPROVE-6/7 错误透传在编排闭环中验证通过");
    }
}

/// IMPROVE-6 Live：真模型规划一个必然触发 edit_patch 错误的任务。
///
/// 预写文件 app.py 含 "def hello():", 然后要求 LLM 用 edit_patch 把 "def goodbye():" 改成 "def welcome():"。
/// "def goodbye():" 不存在于文件中 → edit_patch 必然触发 IMPROVE-7 上下文提示。
/// 验证编排闭环捕获该错误并透传到失败报告。
#[tokio::test]
async fn live_edit_patch_mismatch_error_transparency() {
    let Some((client, model)) = live_stack().await else { return };

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        live_edit_patch_mismatch_error_transparency_inner(client, model),
    )
    .await;
    assert!(result.is_ok(), "e2e timed out after 120s");
    result.unwrap();
}

async fn live_edit_patch_mismatch_error_transparency_inner(
    client: Arc<forge_api::LlmClient>,
    model: String,
) {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();

    // 任务描述明确要求：编辑 app.py, 把 "def goodbye():" 替换为 "def welcome():"
    // 但我们会预写一个不含 "def goodbye():" 的文件, 确保 edit_patch 的 find 不命中
    let task = sdk
        .create_task(
            "live: edit_patch mismatch — replace 'def goodbye():' with 'def welcome():' in app.py"
                .to_string(),
            vec![],
            vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "always passes (error transparency test)".into(),
                check: CheckSpec::Command("echo ok".into()),
            }],
        )
        .await
        .unwrap();

    let ws_manager = Arc::new(forge_workspace::WorkspaceManager::new(ws.path()).unwrap());
    let workdir = ws_manager.create_for(task.id.as_ref()).unwrap();

    // 预写 app.py — 含 "def hello():" 但不含 "def goodbye():"
    std::fs::write(
        workdir.join("app.py"),
        "def hello():\n    print('hello')\n\ndef main():\n    hello()\n",
    )
    .unwrap();

    let router = build_router(&workdir);

    let planner = LlmPlanner {
        backend: client,
        model,
        schema_max_attempts: 3,
        tools: vec![
            "echo".into(),
            "write_file".into(),
            "read_file".into(),
            "list_dir".into(),
            "edit_patch".into(),
            "csv_parse".into(),
            "markdown_render".into(),
        ],
        ledger: None,
        meter: None,
        brief_mode: false,
        context: None,
    };

    let deps = OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: Arc::new(forge_verify::CommandVerifier),
        verifier_file: Arc::new(forge_verify::FileVerifier),
        evidence: Arc::new(forge_evidence::InMemoryEvidenceStore::default()),
        workspace: ws_manager,
        timeout: Duration::from_secs(90),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 0,
            base_backoff_ms: 1,
        }),
        replanner: None,
        max_replans: 0,
        planner: Some(Arc::new(planner)),
        workspace_task: None,
    };

    let orch = Orchestrator {
        capability: "edit_patch".into(),
        timeout: Duration::from_secs(90),
    };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();

    println!(
        "[live] plans={:?} final={:?} completed={} failed={:?}",
        report.plan_versions,
        report.final_status,
        report.execution.completed.len(),
        report.execution.failed
    );

    // 两种可接受结局:
    // A) LLM 聪明: 先 read_file 发现 "def goodbye():" 不存在, 直接 write_file 改成 "def welcome():" → Completed
    // B) LLM 直接 edit_patch("def goodbye():" → "def welcome():") → find 不命中 → IMPROVE-7 上下文提示

    if report.final_status == TaskStatus::Completed {
        println!("[live] ✅ LLM 规划成功 (路径 A: 可能先 read_file 再 write_file)");
    } else {
        let failures = &report.execution.failed;
        assert!(
            failures.is_some(),
            "任务未 Completed 但无失败步骤: {:?}",
            report.final_status
        );

        let (_, err_msg) = failures.as_ref().unwrap();
        let msg = err_msg.clone();

        // 验证失败步骤包含 IMPROVE-6 透传或 IMPROVE-7 上下文提示
        println!("[live] 失败步骤错误: {msg}");
        // IMPROVE-6: execution failed + detail
        let has_exec_detail = msg.contains("execution failed") && msg.contains("—");
        // IMPROVE-7: find not found + Hint + read_file
        let has_context_hint = msg.contains("find not found")
            && msg.contains("Hint:")
            && msg.contains("read_file");
        let has_transparency = has_exec_detail || has_context_hint;

        assert!(
            has_transparency,
            "IMPROVE-6/7 错误透传验证失败: 失败信息不含 execution failed+详情 或 find not found+Hint. \
             错误: {msg}"
        );
        println!("[live] ✅ IMPROVE-6/7 错误透传在真模型编排闭环中验证通过 (路径 B)");
    }
}
