//! V7 BILL-003：编排 LLM 计量全覆盖（build_v70b.md 冻结测试，离线 mock）。

use forge_plan_llm::usage::{LlmMeter, METER_PURPOSES};
use forge_plan_llm::{ChatMessage, LlmPlanBackend, LlmPlanner, LlmReplanner};
use forge_core::ForgeResult;
use forge_recovery::classify::{FailureCategory, FailureRecord};
use forge_planner::{Plan, PlanStatus, Planner as _};
use forge_plan_llm::Replanner as _;
use std::sync::{Arc, Mutex};

/// 收集型计量器。
type UsageLog = Arc<Mutex<Vec<(String, String, u64, u64)>>>;

#[derive(Clone, Default)]
struct CollectingMeter(UsageLog);

impl LlmMeter for CollectingMeter {
    fn on_usage(&self, model: &str, purpose: &str, prompt: u64, completion: u64) {
        self.0.lock().unwrap().push((model.into(), purpose.into(), prompt, completion));
    }
}

/// 带 usage 的 mock 后端（每次调用 (text, Some(usage))）。
struct UsageMock {
    responses: Mutex<Vec<String>>,
    prompt: u64,
    completion: u64,
}

#[async_trait::async_trait]
impl LlmPlanBackend for UsageMock {
    async fn complete(&self, _m: &str, _msgs: &[ChatMessage]) -> ForgeResult<String> {
        let mut g = self.responses.lock().unwrap();
        if g.is_empty() {
            Err(forge_core::ForgeError::InvalidState("mock exhausted".into()))
        } else {
            Ok(g.remove(0))
        }
    }
    async fn complete_with_usage(
        &self,
        model: &str,
        msgs: &[ChatMessage],
    ) -> ForgeResult<(String, Option<forge_plan_llm::usage::TokenUsage>)> {
        let text = self.complete(model, msgs).await?;
        Ok((
            text,
            Some(forge_plan_llm::usage::TokenUsage {
                prompt_tokens: self.prompt,
                completion_tokens: self.completion,
            }),
        ))
    }
}

fn planner_model() -> String {
    r#"{"steps":[{"id":"s1","title":"t","depends_on":[],"action":{"type":"call","capability":"echo","input":{}}}]}"#
        .into()
}

/// 冻结测试：plan 成功 → meter 收 (model, "plan", p, c)。
#[tokio::test]
async fn llm_planner_usage_reaches_meter() {
    let meter = CollectingMeter::default();
    let planner = LlmPlanner {
        backend: Arc::new(UsageMock {
            responses: Mutex::new(vec![planner_model()]),
            prompt: 11,
            completion: 22,
        }),
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
        meter: Some(Arc::new(meter.clone())),
        brief_mode: false,
    };
    let task = forge_task::Task::new(forge_core::TaskId::new_task_id(), "goal".into(), vec![], vec![]);
    planner.plan(&task).await.unwrap();
    let got = meter.0.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "mock");
    assert_eq!(got[0].1, "plan");
    assert_eq!((got[0].2, got[0].3), (11, 22));
}

/// 冻结测试：replan 成功 → purpose "replan"。
#[tokio::test]
async fn replan_usage_reaches_meter() {
    let meter = CollectingMeter::default();
    let replanner = LlmReplanner {
        backend: Arc::new(UsageMock {
            responses: Mutex::new(vec![planner_model()]),
            prompt: 5,
            completion: 6,
        }),
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
        meter: Some(Arc::new(meter.clone())),
    };
    let plan = Plan {
        id: forge_core::PlanId::new_plan_id(),
        task_id: forge_core::TaskId::new_task_id(),
        steps: vec![],
        status: PlanStatus::Failed,
    };
    let failure = FailureRecord {
        id: "fail_x".into(),
        execution_id: forge_core::ExecutionId::new_execution_id(),
        at: chrono::Utc::now(),
        category: FailureCategory::ToolError,
        message: "boom".into(),
        retriable: true,
    };
    replanner.replan(&plan, &[failure]).await.unwrap();
    let got = meter.0.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, "replan");
}

/// 冻结测试：meter=None 时行为与现状一致（plan 成功、无 panic）。
#[tokio::test]
async fn meter_none_is_noop() {
    let planner = LlmPlanner {
        backend: Arc::new(UsageMock {
            responses: Mutex::new(vec![planner_model()]),
            prompt: 1,
            completion: 1,
        }),
        model: "mock".into(),
        schema_max_attempts: 3,
        tools: vec!["echo".into()],
        ledger: None,
        meter: None,
        brief_mode: false,
    };
    let task = forge_task::Task::new(forge_core::TaskId::new_task_id(), "goal".into(), vec![], vec![]);
    let plan = planner.plan(&task).await.unwrap();
    assert_eq!(plan.steps.len(), 1);
}

/// 冻结测试：五 purpose 标签精确集合断言。
#[test]
fn purpose_labels_frozen() {
    assert_eq!(
        METER_PURPOSES,
        ["plan", "replan", "review", "codegen:filename", "codegen:body"]
    );
}
