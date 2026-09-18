//! V8.0 G-V8A 门禁第 2 条：增量开发 e2e（CTX-001 + EDIT-001 闭环）。
//!
//! 验证：任务 A 产出 marker.txt → 任务 B `workspace_task_id` 续作 A 的工作区
//! → mock LLM 规划 [list_dir → edit_patch 精确串替换] 步骤 → 波次执行真正改文件
//! → 验收读改后内容 PASS（工具 root 与验收 workdir 一致指向续作工作区）。

use forge_exec::{
    EditPatchTool, EchoTool, ListDirTool, PermissionLevel, PermissionPolicy, PolicyContext,
    ReadFileTool, ToolRouter, WriteFileTool,
};
use forge_plan_llm::{ChatMessage, LlmPlanBackend, LlmPlanner};
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_workspace::WorkspaceManager;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// mock LLM 返回的增量修改计划：先列目录，再精确串替换。
fn edit_patch_plan() -> String {
    r#"{"steps":[
        {"id":"s1","title":"inspect","depends_on":[],"action":{"type":"call","capability":"list_dir","input":{}}},
        {"id":"s2","title":"edit","depends_on":["s1"],"action":{"type":"call","capability":"edit_patch","input":{"path":"marker.txt","edits":[{"find":"hello world","replace":"hello forge"}]}}}
    ]}"#
    .to_string()
}

/// 冻结测试（G-V8A 第 2 条）：join 作任务 A 的工作区产出，任务 B 续作用 edit_patch 修改，
/// 验收 FileContains 读改后内容 → gate PASS。
#[tokio::test]
async fn resume_then_edit_patch_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let wm = Arc::new(WorkspaceManager::new(tmp.path()).unwrap());

    // —— 任务 A：产出 marker.txt ——
    let ws_a = wm.create_for("TASK-A").unwrap();
    std::fs::write(ws_a.join("marker.txt"), "hello world").unwrap();

    // —— 任务 B：mock 规划器（edit_patch 步骤）+ 全工具 root = 续作工作区 ——
    let mock = MockLlm {
        responses: Mutex::new(vec![edit_patch_plan()]),
    };
    let planner = LlmPlanner {
        backend: Arc::new(mock),
        model: "mock".into(),
        schema_max_attempts: 1,
        tools: vec![
            "echo".into(),
            "write_file".into(),
            "read_file".into(),
            "list_dir".into(),
            "edit_patch".into(),
        ],
        ledger: None,
        meter: None,
        brief_mode: false,
        context: None,
    };

    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    router
        .register(Box::new(ReadFileTool::new(ws_a.clone())))
        .unwrap();
    router
        .register(Box::new(ListDirTool::new(ws_a.clone())))
        .unwrap();
    router
        .register(Box::new(WriteFileTool::new(ws_a.clone())))
        .unwrap();
    router
        .register(Box::new(EditPatchTool::new(ws_a.clone())))
        .unwrap();

    let sdk = ForgeSdk::in_memory();
    let task_b = sdk
        .create_task(
            "edit marker",
            vec![],
            vec![forge_task::AcceptanceCriterion {
                id: "AC-1".into(),
                description: "marker updated".into(),
                check: forge_task::CheckSpec::FileContains {
                    path: "marker.txt".into(),
                    needle: "hello forge".into(),
                },
            }],
        )
        .await
        .unwrap();

    let deps = OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: Arc::new(forge_verify::CommandVerifier),
        verifier_file: Arc::new(forge_verify::FileVerifier),
        evidence: Arc::new(forge_evidence::InMemoryEvidenceStore::default()),
        workspace: wm,
        timeout: Duration::from_secs(15),
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy {
            max_attempts: 1,
            base_backoff_ms: 10,
        }),
        replanner: None,
        max_replans: 1,
        planner: Some(Arc::new(planner) as Arc<dyn forge_planner::Planner>),
        workspace_task: Some("TASK-A".to_string()),
    };
    let orch = Orchestrator {
        capability: "echo".into(),
        timeout: Duration::from_secs(15),
    };

    let report = sdk.run_end_to_end(&task_b.id, &deps, &orch).await.unwrap();
    assert!(report.gate.passed, "增量修改后验收必须 PASS: {:#?}", report.gate);
    // 续作工作区的文件确实被 edit_patch 改掉。
    let content = std::fs::read_to_string(ws_a.join("marker.txt")).unwrap();
    assert_eq!(content, "hello forge", "edit_patch 必须真正替换文件内容");
}