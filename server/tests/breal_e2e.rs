//! B-REAL-001D 真实干活 e2e 测试（测试名冻结 `breal_csv_to_report_e2e`）。
//!
//! 离线 mock 轨：write CSV → read back → csv_parse → markdown_render → write report.html
//! 验证步骤输出引用 $sN.output[.path][|json] 在真实编排中端到端工作。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_exec::{
    EditPatchTool, EchoTool, ListDirTool, PermissionLevel, PermissionPolicy, PolicyContext,
    ReadFileTool, ToolRouter, WriteFileTool,
};
use forge_plan_llm::{ChatMessage, LlmPlanBackend, LlmPlanner};
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_server::builtin_tools::register_builtin_tools;
use forge_task::{AcceptanceCriterion, CheckSpec, TaskStatus};

/// 顺序回放预设响应的离线 mock。
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

/// 5 步计划：write csv → read back → parse → render → emit report
const FIVE_STEP_PLAN: &str = r#"{"steps":[
 {"id":"s1","title":"write csv","depends_on":[],"action":{"type":"call","capability":"write_file",
  "input":{"path":"data.csv","content":"name,score\nalice,91\nbob,72\n"}}},
 {"id":"s2","title":"read back","depends_on":["s1"],"action":{"type":"call","capability":"read_file",
  "input":{"path":"data.csv"}}},
 {"id":"s3","title":"parse","depends_on":["s2"],"action":{"type":"call","capability":"csv_parse",
  "input":{"text":"$s2.output.content"}}},
 {"id":"s4","title":"render","depends_on":["s3"],"action":{"type":"call","capability":"markdown_render",
  "input":{"markdown":"$s3.output.rows|json"}}},
 {"id":"s5","title":"emit report","depends_on":["s4"],"action":{"type":"call","capability":"write_file",
  "input":{"path":"report.html","content":"$s4.output.html"}}}
]}"#;

#[tokio::test]
async fn breal_csv_to_report_e2e() {
    // 护栏：R1-094 教训——异步测试必须带 timeout
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        breal_csv_to_report_e2e_inner(),
    )
    .await;
    assert!(result.is_ok(), "e2e timed out after 60s");
    result.unwrap();
}

async fn breal_csv_to_report_e2e_inner() {
    let ws = tempfile::tempdir().unwrap();
    let sdk = ForgeSdk::in_memory();

    // 验收：FileContains report.html "alice"
    let task = sdk
        .create_task(
            "csv to report e2e",
            vec![],
            vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "report contains alice".into(),
                check: CheckSpec::FileContains {
                    path: "report.html".into(),
                    needle: "alice".into(),
                },
            }],
        )
        .await
        .unwrap();

    // 预创建工作区以获取 workdir（工具 root 与验证 workdir 必须一致）
    let ws_manager =
        Arc::new(forge_workspace::WorkspaceManager::new(ws.path()).unwrap());
    let workdir = ws_manager.create_for(task.id.as_ref()).unwrap();

    // 构建 router：5 基线工具（root=workdir）+ csv_parse + markdown_render
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router
        .register(Box::new(WriteFileTool::new(workdir.clone())))
        .unwrap();
    router
        .register(Box::new(ReadFileTool::new(workdir.clone())))
        .unwrap();
    router
        .register(Box::new(ListDirTool::new(workdir.clone())))
        .unwrap();
    router
        .register(Box::new(EditPatchTool::new(workdir.clone())))
        .unwrap();

    let mut allow = HashSet::new();
    allow.insert("csv_parse".to_string());
    allow.insert("markdown_render".to_string());
    let _ = register_builtin_tools(&router, &allow);

    // MockLlm 返回 5 步计划
    let mock = Arc::new(MockLlm {
        responses: Mutex::new(vec![FIVE_STEP_PLAN.to_string()]),
    });
    let planner = LlmPlanner {
        backend: mock.clone() as Arc<dyn LlmPlanBackend>,
        model: "mock".into(),
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
        timeout: Duration::from_secs(30),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 1,
            base_backoff_ms: 10,
        }),
        replanner: None,
        max_replans: 0,
        planner: Some(Arc::new(planner)),
        workspace_task: None,
    };

    let orch = Orchestrator {
        capability: "csv_parse".into(),
        timeout: Duration::from_secs(30),
    };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await.unwrap();

    // 断言 1：Completed + gate passed
    assert_eq!(
        report.final_status,
        TaskStatus::Completed,
        "final_status must be Completed, failed: {:?}",
        report.execution.failed
    );
    assert!(report.gate.passed, "gate must pass: {:?}", report.gate);

    // 断言 2：5 步完成，非 write_file Success 步骤 ≥ 3（s2/s3/s4）
    assert_eq!(
        report.execution.completed.len(),
        5,
        "all 5 steps must complete"
    );
    for sid in &["s2", "s3", "s4"] {
        assert!(
            report.execution.completed.iter().any(|s| s == sid),
            "step {sid} must be in completed (non-write_file success step)"
        );
    }

    // 断言 3：report.html 存在且内容含 "alice"
    let report_path = workdir.join("report.html");
    assert!(
        report_path.exists(),
        "report.html must exist at {}",
        report_path.display()
    );
    let content = std::fs::read_to_string(&report_path).unwrap();
    assert!(
        content.contains("alice"),
        "report.html must contain 'alice', got: {content}"
    );
}
