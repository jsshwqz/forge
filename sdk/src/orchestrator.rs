//! 端到端编排器（ORCH-002）：把"验证即完成条件"（AP-009）变成一个可调用函数。
//!
//! 流程（CPEVR 闭环）：
//! 1. Task → SequentialPlanner 计划（capability 即工具名）
//! 2. DAG → Scheduler 波次执行，步骤经 ExecutionEngine 真实调用工具
//! 3. 逐条 AcceptanceCriterion → Verifier（Command/File 分派）→ Evidence 固化
//! 4. Gate::evaluate(AllPass) 裁决 → 任务 Completed 或 Failed
//!
//! 全程经 trait 对象操作，内存/PG 栈皆可用；工作目录由 WorkspaceManager 托管。

use crate::ForgeSdk;
use chrono::Utc;
use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_evidence::{Evidence, EvidenceKind, EvidenceStore};
use forge_exec::{ExecutionEngine, ExecutionRequest, PermissionPolicy, ToolRouter};
use forge_gates::{Gate, GateDecision, GatePolicy, GateSpec};
use forge_planner::{Plan, Planner, SequentialPlanner, StepAction};
use forge_scheduler::{run_plan, RunSummary};
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
}

impl ForgeSdk {
    /// 端到端编排：计划→波次执行→验证→证据→门禁→状态迁移。
    ///
    /// 步骤执行失败提前返回时，任务置 Failed 且报告不含验证/门禁数据；
    /// 正常路径下 `gate.passed` 决定 Completed/Failed。
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

        // ---- 1. 计划 ----
        let planner = SequentialPlanner { capability: orch.capability.clone() };
        let plan: Plan = planner.plan(&task).await?;
        let dag = forge_dag::build_dag(&plan)?;

        // ---- 2. 波次执行 ----
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
        let execution: RunSummary = run_plan(&dag, &plan, &step_exec).await?;

        if !execution.succeeded() {
            task.transition(TaskStatus::Failed)?;
            self.tasks.update_status(&task.id, TaskStatus::Failed).await?;
            return Ok(OrchestratorReport {
                task_id: task.id.clone(),
                plan,
                execution,
                verifications: vec![],
                evidence_ids: vec![],
                gate: GateDecision { passed: false, evaluated: 0, missing: vec![], failed: vec![] },
                final_status: TaskStatus::Failed,
                workdir,
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
            plan,
            execution,
            verifications: outcomes,
            evidence_ids,
            gate,
            final_status,
            workdir,
        })
    }
}
