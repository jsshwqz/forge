//! 任务终态 Webhook 通知（V8.0 NOTIFY-001，D12：载荷冻结 + 3 次指数退避）。

use forge_core::TaskId;
use forge_task::TaskStatus;

/// 通知 URL：`FORGE_NOTIFY_URL` 未设或为空 → None（默认关）。
pub fn notify_url() -> Option<String> {
    std::env::var("FORGE_NOTIFY_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// 冻结通知载荷：{task_id, final_status, at, summary}（只投终态，不含敏感内容）。
pub fn notify_payload(task_id: &TaskId, final_status: &TaskStatus) -> serde_json::Value {
    serde_json::json!({
        "task_id": task_id.to_string(),
        "final_status": format!("{final_status:?}"),
        "at": chrono::Utc::now().to_rfc3339(),
        "summary": format!("task {task_id} reached {final_status:?}"),
    })
}

/// 任务终态通知：POST 载荷，失败重试 3 次指数退避（1s/2s/4s）。失败仅 warn 不阻断。
pub async fn notify_task_end(url: &str, task_id: &TaskId, final_status: &TaskStatus) {
    let payload = notify_payload(task_id, final_status);
    let client = reqwest::Client::new();
    let mut delay_secs = 1u64;
    for attempt in 0..3 {
        match client.post(url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(task_id = %task_id, "notify: delivered");
                return;
            }
            Ok(resp) => {
                tracing::warn!(attempt = attempt, status = %resp.status(), "notify: non-2xx");
            }
            Err(e) => {
                tracing::warn!(attempt = attempt, error = %e, "notify: send failed");
            }
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
            delay_secs *= 2;
        }
    }
    tracing::warn!(task_id = %task_id, "notify: exhausted retries, giving up");
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::TaskId;

    #[test]
    fn notify_disabled_by_default() {
        // 默认未设 FORGE_NOTIFY_URL → None。
        let saved = std::env::var("FORGE_NOTIFY_URL").ok();
        std::env::remove_var("FORGE_NOTIFY_URL");
        assert!(notify_url().is_none());
        if let Some(v) = saved {
            std::env::set_var("FORGE_NOTIFY_URL", v);
        }
    }

    #[test]
    fn notify_payload_fields_frozen() {
        let tid = TaskId::from("T-42".to_string());
        let st = TaskStatus::Completed;
        let p = notify_payload(&tid, &st);
        assert_eq!(p["task_id"], "T-42");
        assert_eq!(p["final_status"], "Completed");
        assert!(p["at"].is_string());
        assert!(p["summary"].as_str().unwrap().contains("T-42"));
    }
}
