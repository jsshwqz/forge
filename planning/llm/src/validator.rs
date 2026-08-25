//! PlanSchema 校验器（PLAN-L-001）：LLM 原始 JSON → 合法 Plan。
//!
//! 纯函数，离线可测。错误文案可直接回喂给 LLM 作为修复提示。

use forge_core::{ForgeError, ForgeResult, TaskId};
use forge_planner::{Plan, PlanStatus, StepAction};
use serde_json::Value;

/// 校验 LLM 原始输出并转为合法 Plan。
///
/// 规则：
/// - JSON 解析失败 → InvalidState(具体原因)
/// - 必填字段缺失 → InvalidState(字段名)
/// - step.depends_on 引用悬空 → InvalidState(具体引用)
/// - 通过 build_dag 复用环检测；产出 Plan.status = Ready
pub fn validate_plan(raw: &Value, task_id: &TaskId) -> ForgeResult<Plan> {
    let obj = raw.as_object().ok_or_else(|| {
        ForgeError::InvalidState("plan: root must be a JSON object".into())
    })?;

    // steps 必填且为数组
    let steps = obj.get("steps").and_then(|s| s.as_array()).ok_or_else(|| {
        ForgeError::InvalidState("plan: missing required field 'steps' (must be array)".into())
    })?;

    if steps.is_empty() {
        return Err(ForgeError::InvalidState(
            "plan: 'steps' must not be empty".into(),
        ));
    }

    // 收集所有 step id 用于悬空依赖检查
    let mut all_ids = std::collections::HashSet::new();
    for (i, s) in steps.iter().enumerate() {
        let id = s
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ForgeError::InvalidState(format!(
                    "plan: steps[{}] missing required field 'id'", i
                ))
            })?;
        if !all_ids.insert(id.to_string()) {
            return Err(ForgeError::InvalidState(format!(
                "plan: duplicate step id '{}'", id
            )));
        }
    }

    // 构造 PlanStep 列表
    let mut plan_steps = Vec::new();
    for s in steps.iter() {
        let id = s["id"].as_str().unwrap().to_string();

        let title = s.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let deps: Vec<String> = s
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| d.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // 悬空依赖检查
        for dep in &deps {
            if !all_ids.contains(dep) {
                return Err(ForgeError::InvalidState(format!(
                    "plan: step '{}' depends_on non-existent step '{}'", id, dep
                )));
            }
        }

        let action = match s.get("action") {
            Some(a) => parse_action(a)?,
            None => StepAction::CallCapability {
                capability: "echo".into(),
                input: serde_json::json!({"step": id}),
            },
        };

        plan_steps.push(forge_planner::PlanStep { id, title, depends_on: deps, action });
    }

    let plan = Plan {
        id: forge_core::new_plan_id(),
        task_id: task_id.clone(),
        steps: plan_steps,
        status: PlanStatus::Ready,
    };

    // 环检测复用 build_dag
    forge_dag::build_dag(&plan)?;

    Ok(plan)
}

fn parse_action(v: &Value) -> ForgeResult<StepAction> {
    let kind = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ForgeError::InvalidState("action: missing 'type'".into()))?;

    match kind {
        "call" => {
            let cap = v
                .get("capability")
                .and_then(|c| c.as_str())
                .ok_or_else(|| ForgeError::InvalidState("action.call: missing 'capability'".into()))?;
            let input = v.get("input").cloned().unwrap_or(serde_json::json!({}));
            Ok(StepAction::CallCapability { capability: cap.into(), input })
        }
        "approval" => {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("requires human approval");
            Ok(StepAction::HumanApproval(msg.into()))
        }
        other => Err(ForgeError::InvalidState(format!(
            "action: unknown type '{}', expected 'call' or 'approval'",
            other
        ))),
    }
}

/// 从 LLM 原始输出中提取 JSON 字符串：裸对象直取；``` 围栏取首对之间内容；否则原样返回。
///
/// 供 [`validate_plan`] 的调用方（LlmPlanner / LlmReplanner / Reviewer）解析前统一清洗。
pub fn extract_json_str(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('{') {
        return t.to_string();
    }
    if let Some((_, after_open)) = t.split_once("```") {
        let body = match after_open.split_once("```") {
            Some((inside, _)) => inside.trim(),
            // 只有开围栏没有闭合：取围栏之后的剩余部分
            None => after_open.trim(),
        };
        return body.strip_prefix("json").map(str::trim).unwrap_or(body).to_string();
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid() -> TaskId { TaskId::new_task_id() }

    #[test]
    fn valid_plan_passes() {
        let raw = serde_json::json!({
            "steps": [
                {"id": "s1", "title": "step one"},
                {"id": "s2", "title": "step two", "depends_on": ["s1"]}
            ]
        });
        let plan = validate_plan(&raw, &tid()).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.status, PlanStatus::Ready);
    }

    #[test]
    fn missing_steps_rejected() {
        assert!(validate_plan(&serde_json::json!({}), &tid()).is_err());
    }

    #[test]
    fn empty_steps_rejected() {
        assert!(validate_plan(&serde_json::json!({"steps": []}), &tid()).is_err());
    }

    #[test]
    fn dangling_dependency_rejected() {
        let raw = serde_json::json!({
            "steps": [{"id": "a", "depends_on": ["ghost"]}]
        });
        let err = validate_plan(&raw, &tid()).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn cycle_detected() {
        let raw = serde_json::json!({
            "steps": [
                {"id": "a", "depends_on": ["b"]},
                {"id": "b", "depends_on": ["a"]}
            ]
        });
        assert!(validate_plan(&raw, &tid()).is_err());
    }

    #[test]
    fn duplicate_id_rejected() {
        let raw = serde_json::json!({
            "steps": [{"id": "x"}, {"id": "x"}]
        });
        assert!(validate_plan(&raw, &tid()).is_err());
    }
}
