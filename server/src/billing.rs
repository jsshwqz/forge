//! V6.0 BILL-001：计量流水（build_v60b.md AF-BP-V60B 契约）。
//!
//! 三维度：llm_token / task_count / storage_bytes。只记录不扣费。
//! R4：任何计量失败只 warn 不阻断业务路径。

use forge_core::ForgeResult;
use forge_plan_llm::usage::LlmMeter;
use serde_json::json;

/// 聚合结果行：(kind, sum_quantity)。
pub type UsageSummaryRow = (String, i64);

/// 记一条计量事件（R4：失败仅 warn）。
pub async fn record_usage(
    pool: &sqlx::PgPool,
    tenant: &str,
    kind: &str,
    quantity: i64,
    meta: serde_json::Value,
) {
    let r = sqlx::query(
        "INSERT INTO usage_events (tenant_id, kind, quantity, meta) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant)
    .bind(kind)
    .bind(quantity)
    .bind(meta)
    .execute(pool)
    .await;
    if let Err(e) = r {
        eprintln!("billing: record_usage failed (non-blocking): {e}");
    }
}

/// 聚合查询：按 kind 汇总租户在 [from, to] 的用量。
pub async fn summarize_usage(
    pool: &sqlx::PgPool,
    tenant: &str,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> ForgeResult<Vec<UsageSummaryRow>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, COALESCE(SUM(quantity), 0)::BIGINT AS total FROM usage_events \
         WHERE tenant_id = $1 AND at >= $2 AND at <= $3 GROUP BY kind ORDER BY kind",
    )
    .bind(tenant)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|e| forge_core::ForgeError::InvalidState(format!("usage summarize: {e}")))?;
    Ok(rows)
}



/// PG 落库的 LLM 计量器（tenant 在构造时固定）。
pub struct PgLlmMeter {
    pub pool: sqlx::PgPool,
    pub tenant: String,
}

impl LlmMeter for PgLlmMeter {
    fn on_usage(&self, model: &str, purpose: &str, prompt_tokens: u64, completion_tokens: u64) {
        let pool = self.pool.clone();
        let tenant = self.tenant.clone();
        let model = model.to_string();
        let purpose = purpose.to_string();
        let quantity = i64::try_from(prompt_tokens.saturating_add(completion_tokens)).unwrap_or(i64::MAX);
        tokio::spawn(async move {
            record_usage(
                &pool,
                &tenant,
                "llm_token",
                quantity,
                json!({ "model": model, "purpose": purpose,
                        "prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens }),
            )
            .await;
        });
    }
}

/// 任务工作目录字节占用（R3 storage_bytes 维度：递归累计普通文件大小）。
pub async fn workspace_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&d).await else { continue };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => stack.push(p),
                Ok(ft) if ft.is_file() => {
                    if let Ok(md) = tokio::fs::metadata(&p).await {
                        total += md.len();
                    }
                }
                _ => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冻结测试（BILL-001）：假 meter 收到 (model, purpose, prompt, completion)。
    #[test]
    fn llm_meter_records_token_usage() {
        use std::sync::{Arc, Mutex};
        type UsageLog = Arc<Mutex<Vec<(String, String, u64, u64)>>>;
        struct CollectingMeter(UsageLog);
        impl LlmMeter for CollectingMeter {
            fn on_usage(&self, model: &str, purpose: &str, prompt: u64, completion: u64) {
                self.0.lock().unwrap().push((model.into(), purpose.into(), prompt, completion));
            }
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        let meter = CollectingMeter(log.clone());
        meter.on_usage("mock-model", "codegen", 11, 22);
        let got = log.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "mock-model");
        assert_eq!(got[0].1, "codegen");
        assert_eq!((got[0].2, got[0].3), (11, 22));
    }

    #[tokio::test]
    async fn workspace_bytes_sums_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("a/b");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        tokio::fs::write(tmp.path().join("a/b/x.txt"), "0123456789").await.unwrap();
        tokio::fs::write(tmp.path().join("top.bin"), "01").await.unwrap();
        let total = workspace_bytes(tmp.path()).await;
        assert_eq!(total, 12);
    }
}
