//! V6.0 KNW-101：回归建议沙箱复现验证（build_v60c.md AF-BP-V60C 契约）。
//!
//! 沙箱口径冻结（调用方按此构造 engine）：
//! `ExecutionEngine::new(router, PolicyChain::new().with(Box::new(
//!   AllowListPolicy { allowed: vec![PermissionLevel::ReadOnly] })), InMemorySessionStore)`。
//!
//! 复现判据：执行非 Success 终态且 classify 归一化类别与建议 category 一致；
//! 执行成功或类别不符 → reproduced=false（返回 Ok，不是 Err）。
//! R5 沙箱不可放宽：需非 ReadOnly 权限才能触发的失败模式一律 reproduced=false。

use crate::suggest::RegressionSuggestion;
use forge_core::{ForgeError, ForgeResult, SessionId};
use forge_exec::{ExecutionRequest, ExecutionStatus};
use forge_recovery::classify;
use serde::Serialize;
use std::time::Duration;

/// 单用例执行上限（契约冻结：10s）。
pub const VERIFY_TIMEOUT_SECS: u64 = 10;

/// 验证报告。
#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    pub pattern: String,
    /// sha256(serde_json::to_string(&suggested_case)) hex 小写（与 forge_pr 账本同键）。
    pub case_hash: String,
    pub reproduced: bool,
    /// classify 归一化后的观测类别（执行成功时为 None）。
    pub observed_category: Option<String>,
    pub detail: String,
}

/// 用例内容哈希（账本键；建议改一字节即失配——防"批准后偷换用例"）。
pub fn case_hash(s: &RegressionSuggestion) -> String {
    use sha2::Digest;
    let bytes = serde_json::to_string(&s.suggested_case).unwrap_or_default();
    let mut h = sha2::Sha256::new();
    h.update(bytes.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 期望类别：suggested_case["category"] 优先，回退 pattern 前缀（`<category>:<tool>`）。
fn expected_category(s: &RegressionSuggestion) -> String {
    if let Some(c) = s.suggested_case.get("category").and_then(|v| v.as_str()) {
        return c.to_string();
    }
    s.pattern.split(':').next().unwrap_or("").to_string()
}

/// 沙箱内复现建议用例。
pub async fn verify_suggestion(
    engine: &forge_exec::ExecutionEngine,
    s: &RegressionSuggestion,
) -> ForgeResult<VerifyReport> {
    let tool = s
        .suggested_case
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if tool.is_empty() {
        return Err(ForgeError::InvalidState(
            "verify: suggested_case missing 'tool'".into(),
        ));
    }
    let input = s
        .suggested_case
        .get("input")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let req = ExecutionRequest {
        execution_id: forge_core::new_execution_id(),
        // 会话仅为引擎事件挂载点；引擎对不存在会话的 append 静默忽略（record_result `let _`）
        session_id: SessionId::new_session_id(),
        step_id: "knw_verify".into(),
        tool,
        input,
    };

    let result = tokio::time::timeout(
        Duration::from_secs(VERIFY_TIMEOUT_SECS),
        engine.execute(req),
    )
    .await
    .map_err(|_| ForgeError::InvalidState("verify: case execution timed out".into()))??;

    let report_hash = case_hash(s);
    let expected = expected_category(s);

    if result.status == ExecutionStatus::Success {
        return Ok(VerifyReport {
            pattern: s.pattern.clone(),
            case_hash: report_hash,
            reproduced: false,
            observed_category: None,
            detail: "未复现：执行成功（建议用例未触发失败模式）".into(),
        });
    }

    let msg = result
        .output
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let record = classify(&result.execution_id, result.status, &msg)?;
    let observed = format!("{:?}", record.category);
    let reproduced = observed == expected;
    let detail = if reproduced {
        format!("已复现：status={:?} 类别匹配", result.status)
    } else {
        format!("未复现：观测类别 {observed} 与建议 {expected} 不符（status={:?}）", result.status)
    };

    Ok(VerifyReport {
        pattern: s.pattern.clone(),
        case_hash: report_hash,
        reproduced,
        observed_category: Some(observed),
        detail,
    })
}
