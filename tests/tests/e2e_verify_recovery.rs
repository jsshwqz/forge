//! M3 里程碑门禁：验证与恢复端到端集成测试。
//!
//! 测试剧本：
//! 1. 构造必然失败的工具调用 → ExecutionStatus::Failed
//! 2. classify → ToolError 且 retriable=true
//! 3. BoundedRetryStrategy 三次重试耗尽 → EscalateHuman；Recovery 事件发布成功
//! 4. 用 FileVerifier 验证一条真实标准 → 证据写入 EvidenceStore
//! 5. Gate::evaluate 在缺结果/有失败两种情况下均拒绝放行
//! 6. 全通过路径：验证通过 → Gate passed=true → 任务合法进入 Completed

use forge_core::{ForgeResult, TaskId};
use forge_evidence::{Evidence, EvidenceKind, EvidenceStore, InMemoryEvidenceStore};
use forge_exec::{
    ExecutionEngine, ExecutionRequest, ExecutionStatus, PermissionLevel, PermissionPolicy,
    PolicyContext, Tool, ToolDescriptor, ToolRouter,
};
use forge_event::{EventBus, InMemoryEventBus, Topic};
use forge_gates::{Gate, GatePolicy, GateSpec};
use forge_recovery::{classify, BoundedRetryStrategy, RecoveryAction, RecoveryEngine, RecoveryStrategy};
use forge_session::{InMemorySessionStore, SessionStore};
use forge_task::{AcceptanceCriterion, CheckSpec, InMemoryTaskStore, TaskStatus, TaskStore};
use forge_verify::{FileVerifier, Verdict, VerificationRequest, Verifier};
use std::sync::Arc;

/// AllowAll 策略夹具。
struct AllowAllPolicy;
impl PermissionPolicy for AllowAllPolicy {
    fn check(&self, _level: PermissionLevel, _ctx: &PolicyContext) -> ForgeResult<()> {
        Ok(())
    }
}

/// 失败工具夹具。
struct FailTool;
#[async_trait::async_trait]
impl Tool for FailTool {
    fn descriptor(&self) -> &ToolDescriptor {
        use std::sync::OnceLock;
        static DESC: OnceLock<ToolDescriptor> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptor {
            name: "fail".into(),
            description: "always fails".into(),
            input_schema: serde_json::json!({}),
            permission: PermissionLevel::ReadOnly,
        })
    }
    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Err(forge_core::ForgeError::InvalidState("intentional failure".into()))
    }
}

#[tokio::test]
async fn e2e_verify_recovery() {
    // === 步骤 1：构造必然失败的工具调用 → ExecutionStatus::Failed ===
    let router = Arc::new(ToolRouter::new());
    router.register(Box::new(FailTool)).unwrap();

    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let session = session_store.create(TaskId::new_task_id()).await.unwrap();

    let engine = Arc::new(ExecutionEngine::new(
        router,
        Arc::new(AllowAllPolicy),
        session_store.clone(),
        std::time::Duration::from_secs(5),
    ));

    let req = ExecutionRequest {
        execution_id: forge_core::new_execution_id(),
        session_id: session.id.clone(),
        step_id: "step_1".into(),
        tool: "fail".into(),
        input: serde_json::json!({}),
    };
    let result = engine.execute(req).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Failed);

    // === 步骤 2：classify → ToolError 且 retriable=true ===
    let record = classify(&result.execution_id, result.status, "intentional failure").unwrap();
    assert_eq!(record.category, forge_recovery::FailureCategory::ToolError);
    assert!(record.retriable);

    // === 步骤 3：BoundedRetryStrategy 三次重试耗尽 → EscalateHuman ===
    let strategy = BoundedRetryStrategy::default();
    assert_eq!(strategy.decide(&record, 0), RecoveryAction::Retry { backoff_ms: 1000 });
    assert_eq!(strategy.decide(&record, 1), RecoveryAction::Retry { backoff_ms: 2000 });
    assert_eq!(strategy.decide(&record, 2), RecoveryAction::Retry { backoff_ms: 4000 });
    assert_eq!(strategy.decide(&record, 3), RecoveryAction::EscalateHuman);

    // Recovery 事件发布
    let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());
    let mut rx = event_bus.subscribe(Topic::Recovery).await.unwrap();
    let recovery_engine = RecoveryEngine::new(Arc::new(strategy), event_bus.clone());
    let action = recovery_engine.handle(record, 3).await.unwrap();
    assert_eq!(action, RecoveryAction::EscalateHuman);
    let event = rx.recv().await.unwrap();
    assert_eq!(event.topic, Topic::Recovery);

    // === 步骤 4：FileVerifier 验证 → 证据写入 EvidenceStore ===
    let evidence_store = InMemoryEvidenceStore::default();
    let verifier = FileVerifier;
    let verify_req = VerificationRequest {
        task_id: TaskId::new_task_id(),
        criterion: AcceptanceCriterion {
            id: "AC-1".into(),
            description: "Cargo.toml 存在".into(),
            check: CheckSpec::FileExists("Cargo.toml".into()),
        },
        workdir: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."),
    };
    let outcome = verifier.verify(&verify_req).await.unwrap();
    assert_eq!(outcome.verdict, Verdict::Pass);

    let evidence = Evidence {
        id: forge_core::new_evidence_id(),
        kind: EvidenceKind::FileContent,
        criterion_id: "AC-1".into(),
        content: outcome.reason.clone(),
        produced_by: "FileVerifier".into(),
        at: chrono::Utc::now(),
    };
    evidence_store.put(evidence).await.unwrap();
    let ev = evidence_store.by_criterion("AC-1").await.unwrap();
    assert_eq!(ev.len(), 1);

    // === 步骤 5：Gate 在缺结果/有失败两种情况下均拒绝 ===
    let spec = GateSpec {
        task_id: TaskId::new_task_id(),
        required_criterion_ids: vec!["AC-1".into(), "AC-2".into()],
        policy: GatePolicy::AllPass,
    };

    // 缺结果：只有 AC-1
    let partial_outcomes = vec![outcome.clone()];
    let decision = Gate::evaluate(&spec, &partial_outcomes);
    assert!(!decision.passed);
    assert!(decision.missing.contains(&"AC-2".to_string()));

    // 有失败：AC-1 Pass + AC-2 Fail
    let fail_outcome = forge_verify::VerificationOutcome {
        criterion_id: "AC-2".into(),
        verdict: Verdict::Fail,
        reason: "file not found".into(),
    };
    let mixed_outcomes = vec![outcome.clone(), fail_outcome];
    let decision = Gate::evaluate(&spec, &mixed_outcomes);
    assert!(!decision.passed);
    assert!(decision.failed.contains(&"AC-2".to_string()));

    // === 步骤 6：全通过路径 → Gate passed=true → 任务合法进入 Completed ===
    let pass_outcome_2 = forge_verify::VerificationOutcome {
        criterion_id: "AC-2".into(),
        verdict: Verdict::Pass,
        reason: "ok".into(),
    };
    let all_pass = vec![outcome, pass_outcome_2];
    let decision = Gate::evaluate(&spec, &all_pass);
    assert!(decision.passed);

    // 任务合法进入 Completed
    let task_store = InMemoryTaskStore::default();
    let task = task_store
        .create(
            "test goal".into(),
            vec![],
            vec![
                AcceptanceCriterion {
                    id: "AC-1".into(),
                    description: "c1".into(),
                    check: CheckSpec::FileExists("a.txt".into()),
                },
                AcceptanceCriterion {
                    id: "AC-2".into(),
                    description: "c2".into(),
                    check: CheckSpec::FileExists("b.txt".into()),
                },
            ],
        )
        .await
        .unwrap();

    task_store.update_status(&task.id, TaskStatus::Planned).await.unwrap();
    task_store.update_status(&task.id, TaskStatus::Executing).await.unwrap();
    task_store.update_status(&task.id, TaskStatus::Verifying).await.unwrap();
    task_store.update_status(&task.id, TaskStatus::Completed).await.unwrap();
    let final_task = task_store.get(&task.id).await.unwrap();
    assert_eq!(final_task.status, TaskStatus::Completed);
}
