//! G-V3.1 Live 轨（ORCH-003）：真实模型 × LlmPlanner/LlmReplanner × run_end_to_end。
//!
//! 运行方式（KEY 来自 gitignored .env，勿硬编码入库）：
//! ```powershell
//! $env:FORGE_LLM_LIVE = "1"
//! $env:FORGE_LLM_BASE_URL = "https://token.sensenova.cn/v1"
//! $env:FORGE_LLM_API_KEY  = "<your-key>"
//! cargo test -p forge-sdk --test orchestrator_replan_live -- --nocapture
//! ```
//!
//! 未设置 FORGE_LLM_LIVE=1 时自动跳过（workspace 默认全绿，
//! 供应商配额属外部条件——见 WORKLOG R3-002 / Q-002 / 路线图 R-06）。
//!
//! 契约（roadmap_c.md §4.7）：
//! - 真实模型对含 2 条验收标准的任务产出合法计划，经 run_end_to_end 跑到 Completed；
//! - 构造必失败步骤触发一次 replan，replan 后成功或规范升级 EscalateHuman；
//! - Live 证据按 EvidenceKind::TestReport 固化入库。

use forge_evidence::{Evidence, EvidenceKind, EvidenceStore, InMemoryEvidenceStore};
use forge_api::LlmBackend as _;
use forge_exec::{EchoTool, PermissionPolicy, PermissionLevel, ToolRouter};
use forge_plan_llm::{LlmPlanner, LlmReplanner};
use forge_planner::{Plan, PlanStatus, PlanStep, Planner, StepAction};
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use std::sync::Arc;
use std::time::Duration;

struct AllowAllPolicy;
impl PermissionPolicy for AllowAllPolicy {
    fn check(&self, _: PermissionLevel, _: &forge_exec::PolicyContext) -> forge_core::ForgeResult<()> {
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

fn make_deps(
    planner: Option<Arc<dyn Planner>>,
    replanner: Option<Arc<dyn forge_plan_llm::Replanner>>,
) -> (OrchestratorDeps, Arc<InMemoryEvidenceStore>, tempfile::TempDir) {
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let tmp = tempfile::tempdir().unwrap();
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();
    (
        OrchestratorDeps {
            router: Arc::new(router),
            policy: Arc::new(AllowAllPolicy),
            verifier_cmd: Arc::new(CommandVerifier),
            verifier_file: Arc::new(FileVerifier),
            evidence: evidence.clone(),
            workspace: Arc::new(WorkspaceManager::new(tmp.path()).unwrap()),
            timeout: Duration::from_secs(60),
            recovery: Arc::new(forge_recovery::BoundedRetryStrategy { max_attempts: 0, base_backoff_ms: 1 }),
            replanner,
            max_replans: 1,
            planner,
        },
        evidence,
        tmp,
    )
}

async fn put_live_evidence(evidence: &InMemoryEvidenceStore, name: &str, content: String) {
    let ev = Evidence {
        id: forge_core::new_evidence_id(),
        kind: EvidenceKind::TestReport,
        criterion_id: name.into(),
        content,
        produced_by: "orchestrator_replan_live".into(),
        at: chrono::Utc::now(),
    };
    let id = evidence.put(ev).await.expect("live: put evidence failed");
    println!("[live] TestReport 证据已入库: {id}");
}

/// 场景 1：真实 LLM 规划含 2 条验收标准的任务 → run_end_to_end → Completed。
#[tokio::test]
async fn live_llm_plan_runs_to_completed() {
    let Some((client, model)) = live_stack().await else { return };
    let sdk = ForgeSdk::in_memory();

    let cmd = if cfg!(target_os = "windows") { "echo live-ok> lv.txt" } else { "echo live-ok > lv.txt" };
    let task = sdk
        .create_task(
            "live: llm planned e2e".to_string(),
            vec![],
            vec![
                forge_task::AcceptanceCriterion {
                    id: "AC-1".into(),
                    description: "leave lv.txt behind".into(),
                    check: forge_task::CheckSpec::Command(cmd.into()),
                },
                forge_task::AcceptanceCriterion {
                    id: "AC-2".into(),
                    description: "lv.txt exists".into(),
                    check: forge_task::CheckSpec::FileExists("lv.txt".into()),
                },
            ],
        )
        .await
        .unwrap();

    let planner = Arc::new(LlmPlanner {
        backend: client.clone(),
        model: model.clone(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
    ,
        brief_mode: false
    });
    let replanner = Arc::new(LlmReplanner {
        backend: client.clone(),
        model: model.clone(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
    });
    let (deps, evidence, _tmp) = make_deps(Some(planner), Some(replanner));

    let report = sdk
        .run_end_to_end(
            &task.id,
            &deps,
            &Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(60) },
        )
        .await
        .unwrap();

    println!(
        "[live] plans={:?} final={:?} gate={} replans={} escalated={}",
        report.plan_versions, report.final_status, report.gate.passed, report.replans_used,
        report.escalated_to_human
    );
    println!("[live] exec_failed={:?}", report.execution.failed);
    for v in &report.verifications {
        println!("[live] verify [{}]: {:?} — {}", v.criterion_id, v.verdict, v.reason);
    }
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed, "LLM 计划必须跑到 Completed");
    assert!(report.gate.passed);

    put_live_evidence(
        &evidence,
        "G-V3.1-LIVE-PLAN",
        format!(
            "llm_plan_e2e: task={}, plans={:?}, final={:?}, gate_passed={}",
            task.id, report.plan_versions, report.final_status, report.gate.passed
        ),
    )
    .await;
}

/// 必失败规划器：v1 计划引用不存在的工具（确定性失败），逼出 replan 链路。
struct PoisonFirstPlanner;

#[async_trait::async_trait]
impl Planner for PoisonFirstPlanner {
    async fn plan(&self, task: &forge_task::Task) -> forge_core::ForgeResult<Plan> {
        Ok(Plan {
            id: forge_core::new_plan_id(),
            task_id: task.id.clone(),
            steps: vec![PlanStep {
                id: "step_1".into(),
                title: "poison step (unknown tool)".into(),
                depends_on: vec![],
                action: StepAction::CallCapability {
                    capability: "definitely_missing_tool_v1".into(),
                    input: serde_json::json!({ "goal": task.goal }),
                },
            }],
            status: PlanStatus::Ready,
        })
    }
}

/// 场景 2：必失败步骤触发一次 replan —— 成功或规范升级 EscalateHuman 均算过门禁。
#[tokio::test]
async fn live_forced_failure_replans_or_escalates() {
    let Some((client, model)) = live_stack().await else { return; false
    };
    let sdk = ForgeSdk::in_memory();

    // 验收标准与执行解耦（Command 恒过），专注观察 replan/升级链路
    let task = sdk
        .create_task(
            "live: forced failure then replan".to_string(),
            vec![],
            vec![forge_task::AcceptanceCriterion {
                id: "AC-1".into(),
                description: "always passes".into(),
                check: forge_task::CheckSpec::Command("echo ok".into()),
            }],
        )
        .await
        .unwrap();

    let replanner = Arc::new(LlmReplanner {
        backend: client.clone(),
        model: model.clone(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
    });
    let (deps, evidence, _tmp) = make_deps(Some(Arc::new(PoisonFirstPlanner)), Some(replanner));

    let report = sdk
        .run_end_to_end(
            &task.id,
            &deps,
            &Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(60) },
        )
        .await
        .unwrap();

    println!(
        "[live] plans={:?} final={:?} escalated={} replans={} failure_gate={}",
        report.plan_versions, report.final_status, report.escalated_to_human,
        report.replans_used, report.gate.passed
    );

    let recovered = report.final_status == forge_task::TaskStatus::Completed;
    // 规范升级：预算被真实消耗过（≥1 次 replan 尝试）后仍失败 → 升级人工
    let properly_escalated = report.escalated_to_human && report.replans_used >= 1;
    assert!(
        recovered || properly_escalated,
        "契约要求：replan 后成功 或 规范升级 EscalateHuman"
    );

    put_live_evidence(
        &evidence,
        "G-V3.1-LIVE-REPLAN",
        format!(
            "forced_failure_replan: plans={:?}, replans_used={}, escalated={}, final={:?}",
            report.plan_versions, report.replans_used, report.escalated_to_human, report.final_status
        ),
    )
    .await;
}
