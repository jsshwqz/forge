//! 端到端编排器（ORCH-002）：把"验证即完成条件"（AP-009）变成一个可调用函数。
//!
//! 流程（CPEVR 闭环）：
//! 1. Task → SequentialPlanner 计划（capability 即工具名）
//! 2. DAG → Scheduler 波次执行，步骤经 ExecutionEngine 真实调用工具
//! 3. 失败链路（ORCH-003）：RecoveryStrategy 重试 → 预算内 replan 换新计划续跑 → 耗尽升级人工
//! 4. 逐条 AcceptanceCriterion → Verifier（Command/File 分派）→ Evidence 固化
//! 5. Gate::evaluate(AllPass) 裁决 → 任务 Completed 或 Failed
//!
//! 全程经 trait 对象操作，内存/PG 栈皆可用；工作目录由 WorkspaceManager 托管。

use crate::ForgeSdk;
use chrono::Utc;
use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_evidence::{Evidence, EvidenceKind, EvidenceStore};
use forge_exec::{
    ExecutionEngine, ExecutionRequest, ExecutionStatus, PermissionPolicy, ToolRouter,
};
use forge_gates::{Gate, GateDecision, GatePolicy, GateSpec};
use forge_plan_llm::Replanner;
use forge_planner::{Plan, Planner, SequentialPlanner, StepAction};
use forge_recovery::{
    classify as classify_failure, FailureRecord, RecoveryAction, RecoveryStrategy,
};
use forge_scheduler::{run_plan, RunSummary};
use forge_session::SessionEventKind;
use forge_task::TaskStatus;
use forge_verify::{VerificationOutcome, Verifier, VerificationRequest};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 编排器配置。
#[derive(Clone, Debug)]
pub struct Orchestrator {
    /// CallCapability 所用能力名 = 工具路由中的工具名。
    pub capability: String,
    pub timeout: Duration,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self { capability: "echo".into(), timeout: Duration::from_secs(300) }
    }
}

/// 端到端运行报告。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrchestratorReport {
    pub task_id: TaskId,
    pub plan: Plan,
    pub execution: RunSummary,
    pub verifications: Vec<VerificationOutcome>,
    pub evidence_ids: Vec<forge_core::EvidenceId>,
    pub gate: GateDecision,
    pub final_status: TaskStatus,
    /// 本任务受管工作目录（保留供排障，由调用方决定何时 cleanup）。
    pub workdir: PathBuf,
    /// ORCH-003：全部计划版本 ID（v1 初始在前，其后为每次 replan 产物）。
    pub plan_versions: Vec<String>,
    /// 实际发生的 replan 次数（受 max_replans 预算约束）。
    pub replans_used: u32,
    /// 重规划预算耗尽仍失败 → 已按契约升级人工（EscalateHuman，会话留痕）。
    pub escalated_to_human: bool,
}

/// 单步执行桥：CallCapability → ExecutionEngine；HumanApproval 自动模式不支持。
struct EngineStepExecutor {
    engine: Arc<ExecutionEngine>,
    session_id: forge_core::SessionId,
    step_counter: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl forge_scheduler::StepExecutor for EngineStepExecutor {
    async fn execute(
        &self,
        step_id: &forge_planner::StepId,
        action: &StepAction,
    ) -> ForgeResult<serde_json::Value> {
        match action {
            StepAction::CallCapability { capability, input } => {
                let n =
                    self.step_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
                "orchestrator auto-mode cannot handle HumanApproval: {msg}"
            ))),
        }
    }
}

/// 端到端运行所需的外部依赖集（避免过长参数列表）。
pub struct OrchestratorDeps {
    pub router: Arc<ToolRouter>,
    pub policy: Arc<dyn PermissionPolicy>,
    pub verifier_cmd: Arc<dyn Verifier>,
    pub verifier_file: Arc<dyn Verifier>,
    pub evidence: Arc<dyn EvidenceStore>,
    pub workspace: Arc<forge_workspace::WorkspaceManager>,
    pub timeout: Duration,
    /// 步骤失败后的第一道恢复（既有 RecoveryStrategy，如 BoundedRetryStrategy）。
    /// 恢复仍无法自动修复时才考虑重规划。
    pub recovery: Arc<dyn RecoveryStrategy>,
    /// ORCH-003：可选重规划器。None = 失败即升级（与 ORCH-002 行为兼容）。
    pub replanner: Option<Arc<dyn Replanner>>,
    /// 重规划预算（契约默认 1：步骤失败时最多换一次计划续跑）。
    pub max_replans: u32,
    /// 可选规划器注入（G-V3.1 live 轨 / V3.2 流水线用 LlmPlanner）。
    /// None ⇒ SequentialPlanner 按 capability 机械展开。
    pub planner: Option<Arc<dyn Planner>>,
}

impl ForgeSdk {
    /// 端到端编排：计划→波次执行→验证→证据→门禁→状态迁移。
    ///
    /// 步骤失败时的恢复链（ORCH-003）：
    /// RecoveryStrategy 重试（有界） → 预算内 `deps.replanner` 换新计划续跑
    /// → 预算耗尽仍失败则 EscalateHuman（会话记录全部计划版本号），任务置 Failed。
    /// 执行成功路径不受影响；`gate.passed` 决定 Completed/Failed。
    pub async fn run_end_to_end(
        &self,
        task_id: &TaskId,
        deps: &OrchestratorDeps,
        orch: &Orchestrator,
    ) -> ForgeResult<OrchestratorReport> {
        // ---- 准备 ----
        let mut task = self.tasks.get(task_id).await?;
        let session = self.sessions.create(task.id.clone()).await?;
        let workdir = deps.workspace.create_for(task.id.as_ref())?;

        task.transition(TaskStatus::Planned)?;
        self.tasks.update_status(&task.id, TaskStatus::Planned).await?;
        task.transition(TaskStatus::Executing)?;
        self.tasks.update_status(&task.id, TaskStatus::Executing).await?;

        // ---- 1. 计划（可注入 LLM 规划器；缺省为确定性顺序展开）----
        let plan: Plan = match &deps.planner {
            Some(p) => p.plan(&task).await?,
            None => SequentialPlanner { capability: orch.capability.clone() }.plan(&task).await?,
        };

        // ---- 2. 波次执行（ORCH-003：失败 → RecoveryStrategy → 预算内 replan → 升级人工）----
        let engine = Arc::new(ExecutionEngine::new(
            deps.router.clone(),
            deps.policy.clone(),
            self.sessions.clone(), // 引擎双写事件至同一会话
            deps.timeout,
        ));
        let step_exec = EngineStepExecutor {
            engine,
            session_id: session.id.clone(),
            step_counter: std::sync::atomic::AtomicU64::new(0),
        };

        // 计划版本链：v1 = 初始计划；其后为每次 replan 产物。Session 全程留痕。
        let mut plans: Vec<Plan> = vec![plan];
        let _ = self
            .sessions
            .append(
                &session.id,
                SessionEventKind::PlanCreated,
                serde_json::json!({
                    "plan_id": plans[0].id.to_string(),
                    "version": 1,
                    "origin": "sequential_planner",
                }),
            )
            .await;

        let mut replans_used: u32 = 0;
        let mut recovery_attempts: u32 = 0;
        let mut escalated = false;
        let mut last_failure: Option<FailureRecord> = None;

        let execution: RunSummary = loop {
            let current = plans.last().expect("plan version chain is never empty");
            let dag = forge_dag::build_dag(current)?;
            let summary = run_plan(&dag, current, &step_exec).await?;
            if summary.succeeded() {
                break summary;
            }

            // 失败分类：复用既有确定性规则
            let (failed_step, reason) = {
                let (s, r) = summary.failed.as_ref().expect("not succeeded implies failure");
                (s.clone(), r.clone())
            };
            let record = classify_step_failure(&failed_step, &reason);

            // 第一道：既有 RecoveryStrategy（有界重试，退避由策略决定）
            match deps.recovery.decide(&record, recovery_attempts) {
                RecoveryAction::Retry { backoff_ms } => {
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    recovery_attempts += 1;
                    continue; // 同一计划原样续跑
                }
                RecoveryAction::Skip | RecoveryAction::EscalateHuman => {}
            }

            // 第二道：预算内且配置了重规划器 → 换新计划续跑
            // （契约语义：replan 尝试即消耗预算；LLM 拒绝/校验失败同样烧掉一次机会，
            //   防止对不可救药的计划无限请求重写）
            if replans_used < deps.max_replans {
                if let Some(rp) = deps.replanner.as_ref() {
                    replans_used += 1;
                    match rp.replan(current, std::slice::from_ref(&record)).await {
                        Ok(next) => {
                            recovery_attempts = 0; // 新计划重新起算恢复预算
                            let _ = self
                                .sessions
                                .append(
                                    &session.id,
                                    SessionEventKind::PlanCreated,
                                    serde_json::json!({
                                        "plan_id": next.id.to_string(),
                                        "version": plans.len() + 1,
                                        "origin": "replanner",
                                        "after_failure": record.message,
                                    }),
                                )
                                .await;
                            plans.push(next);
                            continue;
                        }
                        Err(e) => {
                            // 重规划自身失败 → 直接升级（留观测线索）
                            eprintln!(
                                "orchestrator: replan attempt #{replans_used} failed, escalating: {e}"
                            );
                            escalated = true;
                            last_failure = Some(FailureRecord {
                                message: format!("replan failed: {e}"),
                                ..record
                            });
                            break summary;
                        }
                    }
                }
            }

            // 无重规划器 / 预算耗尽 → EscalateHuman
            escalated = true;
            last_failure = Some(record);
            break summary;
        };

        let plan_versions: Vec<String> = plans.iter().map(|p| p.id.to_string()).collect();
        let final_plan = plans.pop().expect("plan version chain is never empty");

        if !execution.succeeded() {
            task.transition(TaskStatus::Failed)?;
            self.tasks.update_status(&task.id, TaskStatus::Failed).await?;

            if escalated {
                // 契约：EscalateHuman 时 Session 记录全部计划版本号
                let _ = self
                    .sessions
                    .append(
                        &session.id,
                        SessionEventKind::Failed,
                        serde_json::json!({
                            "escalated_to_human": true,
                            "replans_used": replans_used,
                            "max_replans": deps.max_replans,
                            "plan_versions": plan_versions,
                            "failure": last_failure.as_ref().map(|f| f.message.clone()),
                        }),
                    )
                    .await;
            }

            return Ok(OrchestratorReport {
                task_id: task.id.clone(),
                plan: final_plan,
                execution,
                verifications: vec![],
                evidence_ids: vec![],
                gate: GateDecision { passed: false, evaluated: 0, missing: vec![], failed: vec![] },
                final_status: TaskStatus::Failed,
                workdir,
                plan_versions,
                replans_used,
                escalated_to_human: escalated,
            });
        }

        // ---- 3. 验证 + 4. 证据 ----
        let mut outcomes = Vec::new();
        let mut evidence_ids = Vec::new();
        for ac in &task.acceptance {
            let req = VerificationRequest {
                task_id: task.id.clone(),
                criterion: ac.clone(),
                workdir: workdir.clone(),
            };
            let (verifier_name, kind, v): (&str, EvidenceKind, &dyn Verifier) = match ac.check {
                forge_task::CheckSpec::Command(_) => (
                    "CommandVerifier",
                    EvidenceKind::CommandOutput,
                    deps.verifier_cmd.as_ref(),
                ),
                _ => ("FileVerifier", EvidenceKind::FileContent, deps.verifier_file.as_ref()),
            };
            let outcome = v.verify(&req).await?;
            let ev = Evidence {
                id: forge_core::new_evidence_id(),
                kind,
                criterion_id: ac.id.clone(),
                content: format!("[{:?}] {}", outcome.verdict, outcome.reason),
                produced_by: verifier_name.into(),
                at: Utc::now(),
            };
            evidence_ids.push(deps.evidence.put(ev).await?);
            outcomes.push(outcome);
        }

        // ---- 5. 门禁 ----
        let spec = GateSpec {
            task_id: task.id.clone(),
            required_criterion_ids: task.acceptance.iter().map(|a| a.id.clone()).collect(),
            policy: GatePolicy::AllPass,
        };
        let gate = Gate::evaluate(&spec, &outcomes);

        // ---- 6. 状态迁移（必须经 Verifying，落实 CPEVR 的 Check 前置）----
        task.transition(TaskStatus::Verifying)?;
        self.tasks.update_status(&task.id, TaskStatus::Verifying).await?;
        let final_status = if gate.passed { TaskStatus::Completed } else { TaskStatus::Failed };
        task.transition(final_status)?;
        self.tasks.update_status(&task.id, final_status).await?;

        Ok(OrchestratorReport {
            task_id: task.id.clone(),
            plan: final_plan,
            execution,
            verifications: outcomes,
            evidence_ids,
            gate,
            final_status,
            workdir,
            plan_versions,
            replans_used,
            escalated_to_human: escalated,
        })
    }
}

/// 从 RunSummary 的失败原因推断 ExecutionStatus 并产出 FailureRecord。
///
/// EngineStepExecutor 的错误文案内嵌 `{:?}` 状态名（Timeout/PermissionDenied/Failed），
/// 此处做确定性文本映射；分类规则复用 forge-recovery，保证与恢复引擎语义一致。
fn classify_step_failure(step_id: &str, reason: &str) -> FailureRecord {
    let status = if reason.contains("Timeout") {
        ExecutionStatus::Timeout
    } else if reason.contains("PermissionDenied") {
        ExecutionStatus::PermissionDenied
    } else {
        ExecutionStatus::Failed
    };
    let eid = forge_core::new_execution_id();
    classify_failure(&eid, status, &format!("step {step_id}: {reason}"))
        .expect("classify cannot fail for non-Success status")
}

#[cfg(test)]
mod replan_tests {
    //! ORCH-003 离线测试：恢复→重规划→升级 的完整决策链。
    //!
    //! 场景（契约 4.6 + 门禁 G-V3.1 离线轨）：
    //! 1. replan_recovers_completed：失败→升级路径→replan→新计划成功→Completed
    //! 2. budget_exhausted_escalates：max_replans=0 → 直接 EscalateHuman
    //! 3. no_replanner_escalates：未配 replanner → 与 ORCH-002 行为一致（失败）
    //! 4. retry_precedes_replan：先按策略重试同计划，重试耗尽后才 replan

    use super::*;
    use forge_exec::{PermissionLevel, Tool, ToolDescriptor, ToolRouter};
    use forge_recovery::BoundedRetryStrategy;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ---------- 夹具 ----------

    struct AllowAllPolicy;
    impl PermissionPolicy for AllowAllPolicy {
        fn check(&self, _: PermissionLevel, _: &forge_exec::PolicyContext) -> ForgeResult<()> {
            Ok(())
        }
    }

    /// 版本闸门工具：仅当 input.attempt >= 2 才成功；统计调用次数。
    struct VersionGateTool {
        desc: ToolDescriptor,
        calls: Arc<AtomicU64>,
    }

    impl VersionGateTool {
        fn new(calls: Arc<AtomicU64>) -> Self {
            Self {
                desc: ToolDescriptor {
                    name: "vgate".into(),
                    description: "succeeds only when input.attempt >= 2".into(),
                    input_schema: serde_json::json!({}),
                    permission: PermissionLevel::WorkspaceWrite,
                },
                calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl Tool for VersionGateTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.desc
        }
        async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let attempt = input.get("attempt").and_then(|v| v.as_u64()).unwrap_or(1);
            if attempt >= 2 {
                Ok(serde_json::json!({"ok": true}))
            } else {
                Err(ForgeError::InvalidState("attempt too low (v1 plan)".into()))
            }
        }
    }

    /// 重规划夹具：所有步骤注入 attempt=2，并换新 Plan.id。
    struct BumpAttemptReplanner;

    #[async_trait::async_trait]
    impl Replanner for BumpAttemptReplanner {
        async fn replan(&self, plan: &Plan, _failures: &[FailureRecord]) -> ForgeResult<Plan> {
            let mut next = plan.clone();
            next.id = forge_core::new_plan_id();
            for s in next.steps.iter_mut() {
                if let StepAction::CallCapability { input, .. } = &mut s.action {
                    if let Some(obj) = input.as_object_mut() {
                        obj.insert("attempt".into(), serde_json::json!(2));
                    }
                }
            }
            Ok(next)
        }
    }

    /// 永远直接升级（把 replan 路径最快暴露出来）。
    struct AlwaysEscalate;
    impl RecoveryStrategy for AlwaysEscalate {
        fn decide(&self, _: &FailureRecord, _: u32) -> RecoveryAction {
            RecoveryAction::EscalateHuman
        }
    }

    /// 先重试一次（1ms 退避），之后升级——验证"恢复先于重规划"的次序。
    struct RetryOnceThenEscalate;
    impl RecoveryStrategy for RetryOnceThenEscalate {
        fn decide(&self, _: &FailureRecord, attempts: u32) -> RecoveryAction {
            if attempts == 0 {
                RecoveryAction::Retry { backoff_ms: 1 }
            } else {
                RecoveryAction::EscalateHuman
            }
        }
    }

    fn criterion_command(id: &str, cmd: &str) -> forge_task::AcceptanceCriterion {
        forge_task::AcceptanceCriterion {
            id: id.into(),
            description: id.into(),
            check: forge_task::CheckSpec::Command(cmd.into()),
        }
    }

    fn make_deps(
        tool: VersionGateTool,
        recovery: Arc<dyn RecoveryStrategy>,
        replanner: Option<Arc<dyn Replanner>>,
        max_replans: u32,
    ) -> (OrchestratorDeps, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let router = ToolRouter::new();
        router.register(Box::new(tool)).unwrap();
        (
            OrchestratorDeps {
                router: Arc::new(router),
                policy: Arc::new(AllowAllPolicy),
                verifier_cmd: Arc::new(forge_verify::CommandVerifier),
                verifier_file: Arc::new(forge_verify::FileVerifier),
                evidence: Arc::new(forge_evidence::InMemoryEvidenceStore::default()),
                workspace: Arc::new(forge_workspace::WorkspaceManager::new(tmp.path()).unwrap()),
                timeout: Duration::from_secs(10),
                recovery,
                replanner,
                max_replans,
                planner: None,
            },
            tmp,
        )
    }

    fn vgate_orch() -> Orchestrator {
        Orchestrator { capability: "vgate".into(), timeout: Duration::from_secs(10) }
    }

    async fn failing_then_fixable_task(sdk: &ForgeSdk) -> forge_core::TaskId {
        let cmd = if cfg!(target_os = "windows") { "echo ok> out.txt" } else { "echo ok > out.txt" };
        let task = sdk
            .create_task("needs replan", vec![], vec![criterion_command("AC-1", cmd)])
            .await
            .unwrap();
        task.id
    }

    // ---------- 场景 ----------

    #[tokio::test]
    async fn replan_recovers_completed() {
        let calls = Arc::new(AtomicU64::new(0));
        let sdk = ForgeSdk::in_memory();
        let task_id = failing_then_fixable_task(&sdk).await;
        let (deps, _tmp) = make_deps(
            VersionGateTool::new(calls.clone()),
            Arc::new(AlwaysEscalate),
            Some(Arc::new(BumpAttemptReplanner)),
            1,
        );

        let report = sdk.run_end_to_end(&task_id, &deps, &vgate_orch()).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Completed);
        assert!(report.gate.passed);
        assert_eq!(report.replans_used, 1);
        assert!(!report.escalated_to_human);
        assert_eq!(report.plan_versions.len(), 2);
        assert_ne!(report.plan_versions[0], report.plan_versions[1]);
        assert_eq!(report.plan.id.to_string(), report.plan_versions[1]);
        // v1 失败一次 + v2 成功一次 = 工具共调用 2 次
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn budget_exhausted_escalates() {
        let calls = Arc::new(AtomicU64::new(0));
        let sdk = ForgeSdk::in_memory();
        let task_id = failing_then_fixable_task(&sdk).await;
        let (deps, _tmp) = make_deps(
            VersionGateTool::new(calls.clone()),
            Arc::new(AlwaysEscalate),
            Some(Arc::new(BumpAttemptReplanner)),
            0, // 预算为 0：即使配置了 replanner 也不得使用
        );

        let report = sdk.run_end_to_end(&task_id, &deps, &vgate_orch()).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Failed);
        assert!(report.escalated_to_human);
        assert_eq!(report.replans_used, 0);
        assert_eq!(report.plan_versions.len(), 1);
        assert!(report.verifications.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1); // 只跑了一轮 v1
    }

    #[tokio::test]
    async fn no_replanner_escalates_like_orch002() {
        let calls = Arc::new(AtomicU64::new(0));
        let sdk = ForgeSdk::in_memory();
        let task_id = failing_then_fixable_task(&sdk).await;
        let (deps, _tmp) = make_deps(VersionGateTool::new(calls.clone()), Arc::new(AlwaysEscalate), None, 1);

        let report = sdk.run_end_to_end(&task_id, &deps, &vgate_orch()).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Failed);
        assert!(report.escalated_to_human);
        assert_eq!(report.replans_used, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_precedes_replan() {
        let calls = Arc::new(AtomicU64::new(0));
        let sdk = ForgeSdk::in_memory();
        let task_id = failing_then_fixable_task(&sdk).await;
        let (deps, _tmp) = make_deps(
            VersionGateTool::new(calls.clone()),
            Arc::new(RetryOnceThenEscalate),
            Some(Arc::new(BumpAttemptReplanner)),
            2,
        );

        let report = sdk.run_end_to_end(&task_id, &deps, &vgate_orch()).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Completed);
        assert_eq!(report.replans_used, 1);
        // 次序断言：v1 失败 → 同计划重试再失败 → replan → v2 成功 = 共 3 次调用
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(report.plan_versions.len(), 2);
    }

    #[tokio::test]
    async fn default_bounded_retry_eventually_escalates_without_replanner() {
        // 默认 BoundedRetryStrategy（3 次、退避 1ms 起步）+ 无 replanner：
        // 4 轮全败后升级。验证退避循环与终态。
        let calls = Arc::new(AtomicU64::new(0));
        let sdk = ForgeSdk::in_memory();
        let task_id = failing_then_fixable_task(&sdk).await;
        let (deps, _tmp) = make_deps(
            VersionGateTool::new(calls.clone()),
            Arc::new(BoundedRetryStrategy { max_attempts: 3, base_backoff_ms: 1 }),
            None,
            1,
        );

        let report = sdk.run_end_to_end(&task_id, &deps, &vgate_orch()).await.unwrap();

        assert_eq!(report.final_status, TaskStatus::Failed);
        assert!(report.escalated_to_human);
        // 重试 3 次 + 首轮 = 4 轮执行，每轮 1 步
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
}
