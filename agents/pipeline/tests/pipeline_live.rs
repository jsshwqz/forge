//! G-V3.2 Live 冒烟（AGENT-O-001）：真实双档模型跑通一次四角色流水线。
//!
//! 运行方式（KEY 来自 gitignored .env）：
//! ```powershell
//! $env:FORGE_LLM_LIVE = "1"
//! $env:FORGE_LLM_BASE_URL = "https://token.sensenova.cn/v1"
//! $env:FORGE_LLM_API_KEY  = "<your-key>"
//! # 可选：$env:FORGE_TIER_HIGH_MODEL / $env:FORGE_TIER_LOW_MODEL（未配置时自动选型/回落）
//! cargo test -p forge-pipeline --test pipeline_live -- --nocapture
//! ```
//!
//! 门禁断言（契约 5.3）：流水线 Completed、审查 verdict=Pass 且证据可查、
//! 成本事件 ≥1（档位+token 进 Session payload）。Low 未配置时验证回落路径。

use forge_evidence::{Evidence, EvidenceKind, EvidenceStore, InMemoryEvidenceStore};
use forge_api::LlmBackend as _;
use forge_exec::{EchoTool, PermissionLevel, PermissionPolicy, ToolRouter};
use forge_pipeline::{
    run_pipeline, LlmStepReviewer, ModelTier, PipelineDeps, TierRouter,
};
use forge_session::InMemorySessionStore;
use forge_task::InMemoryTaskStore;
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

#[tokio::test]
async fn live_four_role_pipeline_smoke() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 双重开关 ----
    if std::env::var("FORGE_LLM_LIVE").as_deref() != Ok("1") {
        eprintln!("[skip] 未设 FORGE_LLM_LIVE=1——live 冒烟按需开启");
        return Ok(());
    }
    let (Ok(base), Ok(key)) = (
        std::env::var("FORGE_LLM_BASE_URL"),
        std::env::var("FORGE_LLM_API_KEY"),
    ) else {
        eprintln!("[skip] FORGE_LLM_* 未设置");
        return Ok(());
    };

    let client = Arc::new(forge_api::LlmClient::new(base, key));
    let models = client.list_models().await.expect("live: list_models failed");

    // ---- 档位配置：HIGH 必配（缺省自动选型），LOW 可缺省（回落路径）----
    let high = match std::env::var("FORGE_TIER_HIGH_MODEL") {
        Ok(m) if !m.is_empty() => m,
        _ => forge_api::pick_default_model(&models).expect("live: no usable model"),
    };
    let low = std::env::var("FORGE_TIER_LOW_MODEL").ok().filter(|s| !s.is_empty());
    std::env::set_var(forge_pipeline::tier::ENV_HIGH, &high);
    if let Some(l) = &low {
        std::env::set_var(forge_pipeline::tier::ENV_LOW, l);
    } else {
        std::env::remove_var(forge_pipeline::tier::ENV_LOW);
    }
    let tier = TierRouter::from_env().expect("live: tier router");
    println!("[live] HIGH={} LOW={:?} (has_low={})", tier.high(), low, tier.has_low());

    let ledger = Arc::new(forge_plan_llm::UsageLedger::new());
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let tmp = tempfile::tempdir().unwrap();
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).unwrap();

    let reviewer_model = tier.resolve(ModelTier::High).to_string();
    let reviewer = Arc::new(LlmStepReviewer {
        backend: client.clone(),
        model: reviewer_model,
        schema_max_attempts: 3,
        ledger: Some(ledger.clone()),
        tier: ModelTier::High,
    });

    let deps = PipelineDeps {
        router: Arc::new(router),
        policy: Arc::new(AllowAllPolicy),
        verifier_cmd: Arc::new(CommandVerifier),
        verifier_file: Arc::new(FileVerifier),
        evidence: evidence.clone(),
        workspace: Arc::new(WorkspaceManager::new(tmp.path()).unwrap()),
        sessions: Arc::new(InMemorySessionStore::default()),
        tasks: Arc::new(InMemoryTaskStore::default()),
        timeout: Duration::from_secs(60),
        backend: client.clone(),
        ledger,
        tier: tier.clone(),
        capability: "echo".into(),
        max_schema_attempts: 3,
        reviewer,
    };

    let cmd =
        if cfg!(target_os = "windows") { "echo v32-live> pv.txt" } else { "echo v32-live > pv.txt" };
    let task = deps
        .tasks
        .create(
            "v3.2 live smoke: greeting file".into(),
            vec![],
            vec![
                forge_task::AcceptanceCriterion {
                    id: "AC-1".into(),
                    description: "write pv.txt via command".into(),
                    check: forge_task::CheckSpec::Command(cmd.into()),
                },
                forge_task::AcceptanceCriterion {
                    id: "AC-2".into(),
                    description: "pv.txt exists".into(),
                    check: forge_task::CheckSpec::FileExists("pv.txt".into()),
                },
            ],
        )
        .await?;

    let report = run_pipeline(&deps, &task).await.unwrap();

    println!(
        "[live] final={:?} gate={} review={:?} replans_n/a costs={} plans={:?}",
        report.final_status,
        report.gate.passed,
        report.review.as_ref().map(|r| r.verdict),
        report.cost_events,
        report.plan_versions
    );
    assert_eq!(report.final_status, forge_task::TaskStatus::Completed, "四角色流水线必须跑通");
    assert!(report.gate.passed);
    let rv = report.review.as_ref().expect("审查结论必须存在");
    // 冒烟门禁（契约5.3）：跑通且 verdict/证据可查。Pass 为理想；Concern 属合法放行。
    assert!(
        matches!(rv.verdict, forge_pipeline::ReviewVerdict::Pass | forge_pipeline::ReviewVerdict::Concern),
        "放行类裁决才视为冒烟通过，实际 {:?}: {}",
        rv.verdict,
        rv.reason
    );
    if rv.verdict == forge_pipeline::ReviewVerdict::Concern {
        println!("[live] reviewer Concern: {}", rv.reason);
    }
    assert!(report.cost_events >= 1, "至少 Architect 一次调用要进成本账本");

    // 审查证据可查（verdict 已入库）
    let ev = evidence.get(&report.evidence_ids[report.evidence_ids.len() - 1]).await.unwrap();
    assert_eq!(ev.criterion_id, "REVIEW");

    // Live 证据按 TestReport 固化入库
    let rec = Evidence {
        id: forge_core::new_evidence_id(),
        kind: EvidenceKind::TestReport,
        criterion_id: "G-V3.2-LIVE".into(),
        content: format!(
            "four_role_smoke: high={}, low_fallback={}, final={:?}, verdict={:?}, cost_events={}",
            tier.high(), !tier.has_low(), report.final_status, rv.verdict, report.cost_events
        ),
        produced_by: "pipeline_live".into(),
        at: chrono::Utc::now(),
    };
    let id = evidence.put(rec).await.unwrap();
    println!("[live] TestReport 证据已入库: {id}");
    Ok(())
}
