//! V7 BILL-003：review 用途计量（forge-pipeline 侧冻结测试，离线 mock）。

use forge_plan_llm::usage::{LlmMeter, TokenUsage};
use forge_plan_llm::{ChatMessage, LlmPlanBackend};
use forge_core::ForgeResult;
use forge_pipeline::{LlmStepReviewer, ModelTier, ReviewInput, StepReviewer as _};
use std::sync::{Arc, Mutex};

/// 收集型计量器。
#[derive(Clone, Default)]
type UsageLog = Arc<Mutex<Vec<(String, String, u64, u64)>>>;
struct CollectingMeter(UsageLog);

impl LlmMeter for CollectingMeter {
    fn on_usage(&self, model: &str, purpose: &str, prompt: u64, completion: u64) {
        self.0.lock().unwrap().push((model.into(), purpose.into(), prompt, completion));
    }
}

/// 返回合法 review JSON 且带 usage 的 mock。
struct UsageMock;

#[async_trait::async_trait]
impl LlmPlanBackend for UsageMock {
    async fn complete(&self, _m: &str, _msgs: &[ChatMessage]) -> ForgeResult<String> {
        Ok(r#"{"verdict":"Pass","reason":"ok"}"#.into())
    }
    async fn complete_with_usage(
        &self,
        model: &str,
        msgs: &[ChatMessage],
    ) -> ForgeResult<(String, Option<TokenUsage>)> {
        let text = self.complete(model, msgs).await?;
        Ok((text, Some(TokenUsage { prompt_tokens: 7, completion_tokens: 8 })))
    }
}

/// 冻结测试：review 成功 → meter 收 purpose "review"。
#[tokio::test]
async fn review_usage_reaches_meter() {
    let meter = CollectingMeter::default();
    let reviewer = LlmStepReviewer {
        backend: Arc::new(UsageMock),
        model: "mock".into(),
        schema_max_attempts: 3,
        ledger: None,
        meter: Some(Arc::new(meter.clone())),
        tier: ModelTier::High,
    };
    let input = ReviewInput::new("review probe").push("step-1", "did the thing");
    let outcome = reviewer.review(&input).await.unwrap();
    let _ = outcome; // verdict 内容由 reviewer 既有测试覆盖，此处只验计量
    let got = meter.0.lock().unwrap();
    assert_eq!(got.len(), 1, "review 必须入账: {got:?}");
    assert_eq!(got[0].0, "mock");
    assert_eq!(got[0].1, "review");
    assert_eq!((got[0].2, got[0].3), (7, 8));
}
