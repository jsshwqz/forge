//! forge-scheduler：波次调度器。
//!
//! 消费 `forge_dag::ready_steps` 的就绪集，按拓扑波次驱动步骤执行：
//! - 每一波内按 StepId 字典序顺序执行（确定性）
//! - 任一步骤失败 → 立即终止，后续波次不再启动
//! - 全部完成 → RunSummary 汇总

use async_trait::async_trait;
use forge_core::ForgeResult;
use forge_dag::{ready_steps, Dag};
use forge_planner::{Plan, StepAction, StepId};
use serde::{Deserialize, Serialize};

/// 步骤执行器（由执行层实现，如 ExecutionEngine 桥接）。
#[async_trait]
pub trait StepExecutor: Send + Sync {
    /// 执行单个步骤，返回输出负载。
    async fn execute(&self, step_id: &StepId, action: &StepAction)
        -> ForgeResult<serde_json::Value>;
}

/// 运行汇总。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    /// 已完成步骤（按完成顺序）。
    pub completed: Vec<StepId>,
    /// 失败步骤及原因；None = 全部成功。
    pub failed: Option<(StepId, String)>,
    /// 实际执行的波次数。
    pub waves: usize,
}

impl RunSummary {
    /// 是否全部成功。
    pub fn succeeded(&self) -> bool {
        self.failed.is_none()
    }
}

/// 按波次执行整个计划（确定性：波内字典序）。
pub async fn run_plan(
    dag: &Dag,
    plan: &Plan,
    exec: &dyn StepExecutor,
) -> ForgeResult<RunSummary> {
    let mut done: std::collections::HashSet<StepId> = std::collections::HashSet::new();
    let mut completed: Vec<StepId> = Vec::new();
    let mut waves = 0usize;

    loop {
        let ready = ready_steps(dag, &done)?;
        if ready.is_empty() {
            break;
        }
        waves += 1;
        for step_id in ready {
            let step = plan
                .steps
                .iter()
                .find(|s| s.id == *step_id)
                .expect("ready step must exist in plan");
            match exec.execute(&step.id, &step.action).await {
                Ok(_) => {
                    done.insert(step_id.clone());
                    completed.push(step_id.clone());
                }
                Err(e) => {
                    return Ok(RunSummary {
                        completed,
                        failed: Some((step.id.clone(), e.to_string())),
                        waves,
                    });
                }
            }
        }
    }

    Ok(RunSummary { completed, failed: None, waves })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{PlanId, TaskId};
    use forge_planner::{Plan, PlanStatus, PlanStep};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn plan_of(steps: Vec<(&str, Vec<&str>)>) -> Plan {
        Plan {
            id: PlanId::new_plan_id(),
            task_id: TaskId::new_task_id(),
            steps: steps
                .into_iter()
                .map(|(id, deps)| PlanStep {
                    id: id.into(),
                    title: id.into(),
                    depends_on: deps.into_iter().map(String::from).collect(),
                    action: StepAction::CallCapability {
                        capability: "cap".into(),
                        input: serde_json::json!({}),
                    },
                })
                .collect(),
            status: PlanStatus::Ready,
        }
    }

    fn build_dag_of(plan: &Plan) -> Dag {
        forge_dag::build_dag(plan).unwrap()
    }

    /// 记录执行顺序；fail_on 中的步骤返回错误。
    struct RecordingExecutor {
        order: Mutex<Vec<String>>,
        fail_on: Vec<String>,
        calls: AtomicUsize,
    }

    impl RecordingExecutor {
        fn new(fail_on: Vec<&str>) -> Self {
            Self {
                order: Mutex::new(Vec::new()),
                fail_on: fail_on.into_iter().map(String::from).collect(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl StepExecutor for RecordingExecutor {
        async fn execute(
            &self,
            step_id: &StepId,
            _action: &StepAction,
        ) -> ForgeResult<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push(step_id.clone());
            if self.fail_on.iter().any(|f| f == step_id) {
                return Err(forge_core::ForgeError::InvalidState(format!(
                    "boom: {step_id}"
                )));
            }
            Ok(serde_json::json!({"ok": true}))
        }
    }

    use std::sync::Mutex;

    #[tokio::test]
    async fn linear_chain_executes_in_order() {
        let plan = plan_of(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["b"])]);
        let dag = build_dag_of(&plan);
        let exec = RecordingExecutor::new(vec![]);
        let summary = run_plan(&dag, &plan, &exec).await.unwrap();

        assert!(summary.succeeded());
        assert_eq!(summary.waves, 3);
        assert_eq!(exec.order.lock().unwrap().clone(), vec!["a", "b", "c"]);
        assert_eq!(summary.completed, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn diamond_respects_wave_boundaries() {
        // a → (b, c) → d
        let plan = plan_of(vec![
            ("a", vec![]),
            ("b", vec!["a"]),
            ("c", vec!["a"]),
            ("d", vec!["b", "c"]),
        ]);
        let dag = build_dag_of(&plan);
        let exec = RecordingExecutor::new(vec![]);
        let summary = run_plan(&dag, &plan, &exec).await.unwrap();

        assert!(summary.succeeded());
        assert_eq!(summary.waves, 3);
        let order = exec.order.lock().unwrap().clone();
        assert_eq!(order[0], "a");
        assert_eq!(*order.last().unwrap(), "d");
        // 中间波次 b/c 都在 a 后 d 前
        let pos = |s: &str| order.iter().position(|x| x == s).unwrap();
        assert!(pos("a") < pos("b") && pos("a") < pos("c"));
        assert!(pos("b") < pos("d") && pos("c") < pos("d"));
    }

    #[tokio::test]
    async fn failure_stops_downstream() {
        let plan = plan_of(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["b"])]);
        let dag = build_dag_of(&plan);
        let exec = Arc::new(RecordingExecutor::new(vec!["b"]));
        let summary = run_plan(&dag, &plan, exec.as_ref()).await.unwrap();

        assert!(!summary.succeeded());
        let (failed_id, msg) = summary.failed.unwrap();
        assert_eq!(failed_id, "b");
        assert!(msg.contains("boom"));
        assert_eq!(summary.completed, vec!["a"]);
        // c 不应被调用
        let order = exec.order.lock().unwrap().clone();
        assert!(!order.contains(&"c".to_string()));
        assert_eq!(exec.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn empty_plan_zero_waves() {
        let plan = plan_of(vec![]);
        let dag = build_dag_of(&plan);
        let exec = RecordingExecutor::new(vec![]);
        let summary = run_plan(&dag, &plan, &exec).await.unwrap();
        assert!(summary.succeeded());
        assert_eq!(summary.waves, 0);
    }
}
