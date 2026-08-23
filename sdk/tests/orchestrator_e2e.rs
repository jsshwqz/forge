//! ORCH-002 集成测试：端到端编排三场景（全离线）。
//!
//! 1. happy_path_completed：echo 步骤 + Command 验收 → Completed + 证据
//! 2. gate_rejected_failed：FileExists 缺失验收 → Fail → Failed
//! 3. exec_failure_short_circuit：未知工具 → 执行失败短路，跳过验证/门禁

use forge_sdk::{
    ForgeSdk, Orchestrator, OrchestratorDeps,
};
use forge_task::TaskStatus;

use forge_exec::{PermissionPolicy, ToolRouter};
use forge_evidence::{EvidenceStore, InMemoryEvidenceStore};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use std::sync::Arc;
use std::time::Duration;

/// AllowAll 策略夹具。
struct AllowAll;

impl PermissionPolicy for AllowAll {
    fn check(
        &self,
        _level: forge_exec::PermissionLevel,
        _ctx: &forge_exec::PolicyContext,
    ) -> forge_core::ForgeResult<()> {
        Ok(())
    }
}

fn deps(register_echo: bool) -> (
    OrchestratorDeps,
    Arc<InMemoryEvidenceStore>,
    Arc<WorkspaceManager>,
    tempfile::TempDir,
) {
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let tmp = tempfile::tempdir().unwrap();
    let ws = Arc::new(WorkspaceManager::new(tmp.path()).unwrap());
    let router = ToolRouter::new();
    if register_echo {
        use forge_exec::EchoTool;
        router.register(Box::new(EchoTool::new())).unwrap();
    }
    let deps = OrchestratorDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAll),
        verifier_cmd: Arc::new(CommandVerifier),
        verifier_file: Arc::new(FileVerifier),
        evidence: evidence.clone(),
        workspace: ws.clone(),
        timeout: Duration::from_secs(10),
    };
    (deps, evidence, ws, tmp)
}

fn orch() -> Orchestrator {
    Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(10) }
}

fn criterion_command(id: &str, cmd: &str) -> forge_task::AcceptanceCriterion {
    forge_task::AcceptanceCriterion {
        id: id.into(),
        description: id.into(),
        check: forge_task::CheckSpec::Command(cmd.into()),
    }
}

#[tokio::test]
async fn happy_path_completed_with_evidence() {
    let sdk = ForgeSdk::in_memory();
    let (deps, evidence, _ws, _tmp) = deps(true);

    // 跨平台命令：在工作目录留下 out.txt（CommandVerifier 自行包装 cmd/sh）
    let cmd = if cfg!(target_os = "windows") {
        "echo hello> out.txt"
    } else {
        "echo hello > out.txt"
    };

    let task = sdk
        .create_task(
            "e2e happy",
            vec![],
            vec![criterion_command("AC-1", cmd)],
        )
        .await
        .unwrap();

    let report = sdk
        .run_end_to_end(&task.id, &deps, &orch())
        .await
        .unwrap();

    assert_eq!(report.final_status, TaskStatus::Completed);
    assert!(report.gate.passed);
    assert_eq!(report.execution.waves, 1);
    assert_eq!(report.verifications.len(), 1);
    println!("DEBUG outcome: {:?}", report.verifications[0]);
    assert_eq!(report.verifications[0].verdict, forge_verify::Verdict::Pass);
    assert_eq!(report.evidence_ids.len(), 1);

    // 证据可回查
    let ev = evidence.get(&report.evidence_ids[0]).await.unwrap();
    assert_eq!(ev.criterion_id, "AC-1");

    // 任务终态经 store 读回一致
    let final_task = sdk.get_task(&task.id).await.unwrap();
    assert_eq!(final_task.status, TaskStatus::Completed);

    // 工作目录保留供排障
    assert!(report.workdir.join("out.txt").exists());
}

#[tokio::test]
async fn gate_rejected_marks_failed() {
    let sdk = ForgeSdk::in_memory();
    let (deps, _ev, _ws, _tmp) = deps(true);

    let task = sdk
        .create_task(
            "e2e reject",
            vec![],
            vec![{
                forge_task::AcceptanceCriterion {
                    id: "AC-MISS".into(),
                    description: "missing file".into(),
                    check: forge_task::CheckSpec::FileExists("never_created.txt".into()),
                }
            }],
        )
        .await
        .unwrap();

    let report = sdk.run_end_to_end(&task.id, &deps, &orch()).await.unwrap();

    assert_eq!(report.final_status, TaskStatus::Failed);
    assert!(!report.gate.passed);
    assert!(report.gate.failed.contains(&"AC-MISS".to_string()));
    assert_eq!(report.verifications[0].verdict, forge_verify::Verdict::Fail);

    let final_task = sdk.get_task(&task.id).await.unwrap();
    assert_eq!(final_task.status, TaskStatus::Failed);
}

#[tokio::test]
async fn exec_failure_short_circuits_verification() {
    let sdk = ForgeSdk::in_memory();
    let (deps, _ev, _ws, _tmp) = deps(false);

    // 路由为空 → capability "echo" 不存在 → 引擎返回 Failed
    let task = sdk
        .create_task("e2e short-circuit", vec![], vec![])
        .await
        .unwrap();

    let report = sdk.run_end_to_end(&task.id, &deps, &orch()).await.unwrap();

    assert_eq!(report.final_status, TaskStatus::Failed);
    assert!(report.execution.failed.is_some(), "execution must record failure");
    assert!(report.verifications.is_empty(), "verification must be skipped");
    assert!(!report.gate.passed);

    let final_task = sdk.get_task(&task.id).await.unwrap();
    assert_eq!(final_task.status, TaskStatus::Failed);
}
