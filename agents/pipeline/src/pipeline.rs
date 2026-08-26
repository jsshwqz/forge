//! 四角色流水线编排（AGENT-O-001）。
//!
//! 流程（契约 5.2）：Architect(高档 LLM 出计划) → Builder(确定性工具执行波次)
//! → Tester(Verifier 验收 + 证据固化) → Reviewer(LLM 审查) → Gate。
//!
//! 规则：
//! - Reviewer `Reject` 一票否决 Gate → 任务 Failed 且升级人工（EscalateHuman）；
//! - `Concern` 放行但审查结论照常入库；
//! - Builder 阶段失败同样升级人工（流水线 v1 不做 replan，见 R6-015 的范围决策）；
//! - 成本记录：所有 LLM 调用经 [`forge_plan_llm::UsageLedger`] 汇总，
//!   收尾时逐条写入 Session payload（G-V3.2 要求 token 数可查）。

use crate::reviewer::{ReviewInput, StepReviewer};
use crate::role::{default_profiles, ModelTier};
use crate::tier::TierRouter;
use forge_core::{ForgeError, ForgeResult};
use forge_evidence::{Evidence, EvidenceKind, EvidenceStore};
use forge_exec::{ExecutionEngine, ExecutionRequest, PermissionPolicy, ToolRouter};
use forge_gates::{Gate, GateDecision, GatePolicy, GateSpec};
use forge_plan_llm::{LlmPlanBackend, LlmPlanner, UsageLedger};
use forge_planner::{Plan, Planner, StepAction};
use forge_scheduler::{run_plan, RunSummary};
use forge_session::{SessionEventKind, SessionStore};
use forge_task::{Task, TaskStatus, TaskStore};
use forge_verify::{VerificationRequest, Verifier};
use forge_workspace::WorkspaceManager;
use std::sync::Arc;
use std::time::Duration;

/// 流水线依赖集。
pub struct PipelineDeps {
    pub router: Arc<ToolRouter>,
    pub policy: Arc<dyn PermissionPolicy>,
    pub verifier_cmd: Arc<dyn Verifier>,
    pub verifier_file: Arc<dyn Verifier>,
    pub evidence: Arc<dyn EvidenceStore>,
    pub workspace: Arc<WorkspaceManager>,
    pub sessions: Arc<dyn SessionStore>,
    pub tasks: Arc<dyn TaskStore>,
    pub timeout: Duration,
    /// 共享 LLM 后端；档位仅决定模型名（R6 决策，见 tier 模块）。
    pub backend: Arc<dyn LlmPlanBackend>,
    /// 成本账本（planner 与调用方共享；reviewer 若需入账请复用同一实例）。
    pub ledger: Arc<UsageLedger>,
    /// 分层路由器。
    pub tier: TierRouter,
    /// Builder 步骤调用的工具名（CallCapability.capability）。
    pub capability: String,
    /// LLM 输出 schema 修复重试上限。
    pub max_schema_attempts: u32,
    /// 审查者（生产用 LlmStepReviewer；测试注入 mock）。
    pub reviewer: Arc<dyn StepReviewer>,
}

/// 流水线运行报告。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PipelineReport {
    pub task_id: forge_core::TaskId,
    pub plan: Plan,
    pub plan_versions: Vec<String>,
    pub execution: RunSummary,
    pub verifications: Vec<forge_verify::VerificationOutcome>,
    /// 审查结论（Tester 前失败时为 None）。
    pub review: Option<crate::reviewer::ReviewOutcome>,
    pub evidence_ids: Vec<forge_core::EvidenceId>,
    pub gate: GateDecision,
    pub final_status: TaskStatus,
    pub escalated_to_human: bool,
    /// 写进 Session 的成本事件条数。
    pub cost_events: usize,
}

/// Builder 桥：CallCapability → ExecutionEngine（与 sdk 编排器同语义）。
struct EngineStepBridge {
    engine: Arc<ExecutionEngine>,
    session_id: forge_core::SessionId,
    counter: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl forge_scheduler::StepExecutor for EngineStepBridge {
    async fn execute(
        &self,
        step_id: &forge_planner::StepId,
        action: &StepAction,
    ) -> ForgeResult<serde_json::Value> {
        match action {
            StepAction::CallCapability { capability, input } => {
                let n = self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let req = ExecutionRequest {
                    execution_id: forge_core::new_execution_id(),
                    session_id: self.session_id.clone(),
                    step_id: format!("{step_id}#{n}"),
                    tool: capability.clone(),
                    input: input.clone(),
                };
                let result = self.engine.execute(req).await?;
                if result.status != forge_exec::ExecutionStatus::Success {
                    return Err(ForgeError::InvalidState(format!(
                        "step {step_id} execution failed: {:?}",
                        result.status
                    )));
                }
                Ok(result.output)
            }
            StepAction::HumanApproval(msg) => Err(ForgeError::InvalidState(format!(
                "pipeline auto-mode cannot handle HumanApproval: {msg}"
            ))),
        }
    }
}

async fn set_status(
    task: &mut Task,
    tasks: &dyn TaskStore,
    st: TaskStatus,
) -> ForgeResult<()> {
    task.transition(st)?;
    tasks.update_status(&task.id, st).await.map(|_| ())
}

/// 运行四角色流水线。入口时任务应处于 Pending（内部走 Planned→Executing→…）。
pub async fn run_pipeline(
    deps: &PipelineDeps,
    task: &Task,
) -> ForgeResult<PipelineReport> {
    let mut task = task.clone();
    let session = deps.sessions.create(task.id.clone()).await?;
    let workdir = deps.workspace.create_for(task.id.as_ref())?;

    // 角色档案留痕（可追溯本次流水线的分层策略）
    let profiles = default_profiles();
    let _ = deps
        .sessions
        .append(
            &session.id,
            SessionEventKind::TaskReceived,
            serde_json::json!({
                "stage": "pipeline.start",
                "roles": profiles.iter()
                    .map(|p| serde_json::json!({
                        "role": p.role.as_str(),
                        "tier": format!("{:?}", p.model_tier),
                        "model": deps.tier.resolve(p.model_tier),
                    }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;

    set_status(&mut task, deps.tasks.as_ref(), TaskStatus::Planned).await?;
    set_status(&mut task, deps.tasks.as_ref(), TaskStatus::Executing).await?;

    // ---- 1. Architect：高档模型出计划 ----
    let architect = LlmPlanner {
        backend: deps.backend.clone(),
        model: deps.tier.resolve(ModelTier::High).to_string(),
        schema_max_attempts: deps.max_schema_attempts,
        tools: vec![deps.capability.clone()],
        ledger: Some(deps.ledger.clone()),
        brief_mode: false
    };
    let plan: Plan = architect.plan(&task).await?;
    let _ = deps
        .sessions
        .append(
            &session.id,
            SessionEventKind::PlanCreated,
            serde_json::json!({
                "plan_id": plan.id.to_string(),
                "version": 1,
                "origin": "pipeline.architect",
            }),
        )
        .await;
    let plan_versions = vec![plan.id.to_string()];

    // ---- 2. Builder：确定性工具执行（无 replan，失败即升级——v1 范围决策）----
    let engine = Arc::new(ExecutionEngine::new(
        deps.router.clone(),
        deps.policy.clone(),
        deps.sessions.clone(),
        deps.timeout,
    ));
    let bridge = EngineStepBridge {
        engine,
        session_id: session.id.clone(),
        counter: std::sync::atomic::AtomicU64::new(0),
    };
    let dag = forge_dag::build_dag(&plan)?;
    let execution: RunSummary = run_plan(&dag, &plan, &bridge).await?;

    if !execution.succeeded() {
        set_status(&mut task, deps.tasks.as_ref(), TaskStatus::Failed).await?;
        let reason = execution
            .failed
            .as_ref()
            .map(|(s, r)| format!("step {s}: {r}"))
            .unwrap_or_default();
        // 成本事件先落，再写终态失败事件（防会话状态机回跳）
        let cost_events = flush_costs(deps, &session.id).await;
        let _ = deps
            .sessions
            .append(
                &session.id,
                SessionEventKind::Failed,
                serde_json::json!({
                    "stage": "builder",
                    "escalated_to_human": true,
                    "reason": reason,
                }),
            )
            .await;
        return Ok(PipelineReport {
            task_id: task.id,
            plan,
            plan_versions,
            execution,
            verifications: vec![],
            review: None,
            evidence_ids: vec![],
            gate: GateDecision { passed: false, evaluated: 0, missing: vec![], failed: vec![] },
            final_status: TaskStatus::Failed,
            escalated_to_human: true,
            cost_events,
        });
    }

    // ---- 3. Tester：Verifier 验收 + 证据固化 ----
    let mut outcomes = Vec::new();
    let mut evidence_ids = Vec::new();
    for ac in &task.acceptance {
        let req = VerificationRequest {
            task_id: task.id.clone(),
            criterion: ac.clone(),
            workdir: workdir.clone(),
        };
        let (name, kind, v): (&str, EvidenceKind, &dyn Verifier) = match ac.check {
            forge_task::CheckSpec::Command(_) => {
                ("CommandVerifier", EvidenceKind::CommandOutput, deps.verifier_cmd.as_ref())
            }
            _ => ("FileVerifier", EvidenceKind::FileContent, deps.verifier_file.as_ref()),
        };
        let outcome = v.verify(&req).await?;
        let ev = Evidence {
            id: forge_core::new_evidence_id(),
            kind,
            criterion_id: ac.id.clone(),
            content: format!("[{:?}] {}", outcome.verdict, outcome.reason),
            produced_by: name.into(),
            at: chrono::Utc::now(),
        };
        evidence_ids.push(deps.evidence.put(ev).await?);
        outcomes.push(outcome);
    }

    // ---- 4. Reviewer：LLM 审查（高档）----
    let mut input = ReviewInput::new(task.goal.clone());
    input = input.push(
        "builder",
        format!(
            "{} steps completed across {} waves",
            execution.completed.len(),
            execution.waves
        ),
    );
    for o in &outcomes {
        input = input.push(o.criterion_id.clone(), format!("{:?}: {}", o.verdict, o.reason));
    }
    let review = deps.reviewer.review(&input).await?;
    let review_ev = Evidence {
        id: forge_core::new_evidence_id(),
        kind: EvidenceKind::Log,
        criterion_id: "REVIEW".into(),
        content: format!("[{:?}] {}", review.verdict, review.reason),
        produced_by: "reviewer".into(),
        at: chrono::Utc::now(),
    };
    evidence_ids.push(deps.evidence.put(review_ev).await?);

    // ---- 5. Gate（AllPass）+ Reviewer 一票否决 ----
    let spec = GateSpec {
        task_id: task.id.clone(),
        required_criterion_ids: task.acceptance.iter().map(|a| a.id.clone()).collect(),
        policy: GatePolicy::AllPass,
    };
    let gate = Gate::evaluate(&spec, &outcomes);
    let vetoed = review.verdict == crate::reviewer::ReviewVerdict::Reject;

    set_status(&mut task, deps.tasks.as_ref(), TaskStatus::Verifying).await?;
    let final_status =
        if gate.passed && !vetoed { TaskStatus::Completed } else { TaskStatus::Failed };
    set_status(&mut task, deps.tasks.as_ref(), final_status).await?;

    let escalated = vetoed || !gate.passed;
    if escalated {
        // 成本事件先落，再写终态失败事件（防会话状态机回跳）
        let _ = flush_costs(deps, &session.id).await;
        let _ = deps
            .sessions
            .append(
                &session.id,
                SessionEventKind::Failed,
                serde_json::json!({
                    "stage": if vetoed { "reviewer" } else { "gate" },
                    "escalated_to_human": true,
                    "verdict": format!("{:?}", review.verdict),
                    "reason": review.reason,
                }),
            )
            .await;
    }
    let cost_events = if escalated { 0 } else { flush_costs(deps, &session.id).await };

    Ok(PipelineReport {
        task_id: task.id,
        plan,
        plan_versions,
        execution,
        verifications: outcomes,
        review: Some(review),
        evidence_ids,
        gate,
        final_status,
        escalated_to_human: escalated,
        cost_events,
    })
}

/// 把账本余额写进 Session payload，返回条数。
async fn flush_costs(deps: &PipelineDeps, session_id: &forge_core::SessionId) -> usize {
    let entries = deps.ledger.drain();
    let n = entries.len();
    for e in entries {
        let _ = deps
            .sessions
            .append(
                session_id,
                SessionEventKind::ActionResult,
                serde_json::json!({ "cost": {
                    "model": e.model,
                    "purpose": e.purpose,
                    "prompt_tokens": e.prompt_tokens,
                    "completion_tokens": e.completion_tokens,
                }}),
            )
            .await;
    }
    n
}

#[cfg(test)]
mod tests {
    //! G-V3.2 离线轨：Mock LLM + Mock Reviewer 的流水线全链路。

    use super::*;
    use crate::reviewer::{ReviewOutcome, ReviewVerdict};
    use forge_evidence::InMemoryEvidenceStore;
    use forge_exec::{EchoTool, PermissionLevel};
    use forge_session::InMemorySessionStore;
    use forge_task::InMemoryTaskStore;
    use forge_verify::{CommandVerifier, FileVerifier};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct AllowAll;
    impl PermissionPolicy for AllowAll {
        fn check(&self, _: PermissionLevel, _: &forge_exec::PolicyContext) -> ForgeResult<()> {
            Ok(())
        }
    }

    /// 顺序回放预设响应的 mock（无 usage —— 验证账本对 None 容错）。
    struct MockBackend {
        responses: Vec<String>,
        calls: AtomicU64,
    }

    #[async_trait::async_trait]
    impl LlmPlanBackend for MockBackend {
        async fn complete(&self, _m: &str, _msgs: &[forge_plan_llm::ChatMessage]) -> ForgeResult<String> {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses.get(i as usize).cloned().ok_or_else(|| {
                ForgeError::InvalidState("mock: exhausted".into())
            })
        }
    }

    /// 固定裁决的 mock 审查者。
    struct FixedReviewer {
        verdict: ReviewVerdict,
        calls: Arc<AtomicU64>,
    }

    #[async_trait::async_trait]
    impl StepReviewer for FixedReviewer {
        async fn review(&self, _input: &ReviewInput) -> ForgeResult<ReviewOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ReviewOutcome { verdict: self.verdict, reason: "mock review".into() })
        }
    }

    const GOOD_PLAN: &str = r#"{"steps":[{"id":"s1","title":"t","depends_on":[]}]}"#;

    fn make_deps(
        backend: MockBackend,
        reviewer: FixedReviewer,
    ) -> (PipelineDeps, Arc<UsageLedger>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let router = ToolRouter::new();
        router.register(Box::new(EchoTool::new())).unwrap();
        let ledger = Arc::new(UsageLedger::new());
        let tier = TierRouter::from_parts("high-model", Some("low-model".into()));
        (
            PipelineDeps {
                router: Arc::new(router),
                policy: Arc::new(AllowAll),
                verifier_cmd: Arc::new(CommandVerifier),
                verifier_file: Arc::new(FileVerifier),
                evidence: Arc::new(InMemoryEvidenceStore::default()),
                workspace: Arc::new(WorkspaceManager::new(tmp.path()).unwrap()),
                sessions: Arc::new(InMemorySessionStore::default()),
                tasks: Arc::new(InMemoryTaskStore::default()),
                timeout: Duration::from_secs(10),
                backend: Arc::new(backend),
                ledger: ledger.clone(),
                tier,
                capability: "echo".into(),
                max_schema_attempts: 3,
                reviewer: Arc::new(reviewer),
            },
            ledger,
            tmp,
        )
    }

    async fn fixture_task(deps: &PipelineDeps) -> Task {
        let cmd = if cfg!(target_os = "windows") { "echo hi> out.txt" } else { "echo hi > out.txt" };
        let t = Task::new(
            forge_core::TaskId::new_task_id(),
            "pipeline demo".into(),
            vec![],
            vec![forge_task::AcceptanceCriterion {
                id: "AC-1".into(),
                description: "leave out.txt".into(),
                check: forge_task::CheckSpec::Command(cmd.into()),
            }],
        );
        deps.tasks.create(t.goal.clone(), t.constraints.clone(), t.acceptance.clone()).await.unwrap()
    }

    fn mock_backend_one_shot() -> MockBackend {
        MockBackend { responses: vec![GOOD_PLAN.to_string()], calls: AtomicU64::new(0) }
    }

    #[tokio::test]
    async fn pass_flow_completes_with_review_and_costs() {
        let backend = mock_backend_one_shot();
        let calls = Arc::new(AtomicU64::new(0));
        let reviewer = FixedReviewer { verdict: ReviewVerdict::Pass, calls: calls.clone() };
        let (deps, ledger, _tmp) = make_deps(backend, reviewer);
        let task = fixture_task(&deps).await;

        // 模拟 planner 的一次带用量调用（真实记账由 LlmPlanner 完成；
        // 这里直接预置一条账目验证落盘链路）
        ledger.record(forge_plan_llm::CostEntry {
            model: "high-model".into(),
            purpose: "plan".into(),
            prompt_tokens: 10,
            completion_tokens: 5,
        });

        let report = run_pipeline(&deps, &task).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Completed);
        assert!(report.gate.passed);
        assert!(!report.escalated_to_human);
        assert_eq!(report.review.as_ref().unwrap().verdict, ReviewVerdict::Pass);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(report.cost_events, 1);
        // REVIEW 证据在列（AC-1 + REVIEW 至少两条）
        assert!(report.evidence_ids.len() >= 2);
    }

    #[tokio::test]
    async fn reject_verdict_vetoes_gate() {
        let backend = mock_backend_one_shot();
        let calls = Arc::new(AtomicU64::new(0));
        let reviewer = FixedReviewer { verdict: ReviewVerdict::Reject, calls: calls.clone() };
        let (deps, _ledger, _tmp) = make_deps(backend, reviewer);
        let task = fixture_task(&deps).await;

        let report = run_pipeline(&deps, &task).await.unwrap();

        // 验收全过但审查否决 → 一票否决生效
        assert!(report.gate.passed, "AC 本身应通过");
        assert_eq!(report.review.as_ref().unwrap().verdict, ReviewVerdict::Reject);
        assert_eq!(report.final_status, TaskStatus::Failed);
        assert!(report.escalated_to_human);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concern_allows_completion() {
        let backend = mock_backend_one_shot();
        let reviewer =
            FixedReviewer { verdict: ReviewVerdict::Concern, calls: Arc::new(AtomicU64::new(0)) };
        let (deps, _l, _t) = make_deps(backend, reviewer);
        let task = fixture_task(&deps).await;

        let report = run_pipeline(&deps, &task).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Completed);
        assert!(!report.escalated_to_human);
        assert_eq!(report.review.as_ref().unwrap().verdict, ReviewVerdict::Concern);
    }

    #[tokio::test]
    async fn builder_failure_escalates_without_review() {
        // 计划引用不存在的工具 → Builder 失败 → 升级；Reviewer 不应被调用
        let bad = r#"{"steps":[{"id":"s1","title":"t","depends_on":[],
            "action":{"type":"call","capability":"nope","input":{}}}]}"#;
        let backend = MockBackend { responses: vec![bad.to_string()], calls: AtomicU64::new(0) };
        let calls = Arc::new(AtomicU64::new(0));
        let reviewer = FixedReviewer { verdict: ReviewVerdict::Pass, calls: calls.clone() };
        let (deps, _l, _t) = make_deps(backend, reviewer);
        let task = fixture_task(&deps).await;

        let report = run_pipeline(&deps, &task).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Failed);
        assert!(report.escalated_to_human);
        assert!(report.execution.failed.is_some());
        assert!(report.verifications.is_empty());
        assert!(report.review.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "builder 失败后不得进入审查");
    }

    #[tokio::test]
    async fn architect_invalid_output_fails_closed() {
        // LLM 三次都输出垃圾 → Architect 校验耗尽 → 流水线失败（不 panic、不留半态）
        let backend =
            MockBackend { responses: vec!["garbage".into(), "garbage".into(), "garbage".into()], calls: AtomicU64::new(0) };
        let calls = Arc::new(AtomicU64::new(0));
        let reviewer = FixedReviewer { verdict: ReviewVerdict::Pass, calls: calls.clone() };
        let (deps, _l, _t) = make_deps(backend, reviewer);
        let task = fixture_task(&deps).await;

        let err = run_pipeline(&deps, &task).await;
        assert!(err.is_err(), "schema 耗尽必须报错");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "架构失败时不得进入审查");
    }
}

