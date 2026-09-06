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

// ==================== BILL-002：费率与账单 ====================

use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 设置/覆盖租户费率。
pub async fn set_rate(
    pool: &sqlx::PgPool,
    tenant: &str,
    kind: &str,
    unit_price_micros: i64,
    currency: &str,
) -> ForgeResult<()> {
    sqlx::query(
        "INSERT INTO rates (tenant_id, kind, unit_price_micros, currency) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (tenant_id, kind) DO UPDATE SET unit_price_micros = $3, currency = $4",
    )
    .bind(tenant)
    .bind(kind)
    .bind(unit_price_micros)
    .bind(currency)
    .execute(pool)
    .await
    .map_err(|e| forge_core::ForgeError::InvalidState(format!("set_rate: {e}")))?;
    Ok(())
}

/// 列出租户费率：(kind, unit_price_micros, currency)，按 kind 字典序。
pub async fn list_rates(pool: &sqlx::PgPool, tenant: &str) -> ForgeResult<Vec<(String, i64, String)>> {
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT kind, unit_price_micros, currency FROM rates WHERE tenant_id = $1 ORDER BY kind",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .map_err(|e| forge_core::ForgeError::InvalidState(format!("list_rates: {e}")))?;
    Ok(rows)
}

/// 账单行。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct BillLine {
    pub kind: String,
    pub quantity: i64,
    pub unit_price_micros: i64,
    pub amount_micros: i64,
    pub currency: String,
}

/// 账单文档（字段序固定 = 序列化字节确定；R2 全整数微货币）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct BillDoc {
    pub tenant_id: String,
    pub period_from: String,
    pub period_to: String,
    pub currency: String,
    pub usage_hash: String,
    pub lines: Vec<BillLine>,
    pub unrated: Vec<String>,
    pub total_micros: i64,
}

/// 生成/重算账单（R3 幂等：同流水 + 同费率 → doc 字节一致、doc_hash 一致）。
///
/// R1 费率缺失的 kind 计 0 金额并入 unrated 列；R19 doc 附 usage_hash。
pub async fn generate_bill(
    pool: &sqlx::PgPool,
    tenant: &str,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> ForgeResult<(i64, String, BillDoc)> {
    let usage = summarize_usage(pool, tenant, from, to).await?;
    let rates = list_rates(pool, tenant).await?;

    // usage_hash：流水聚合哈希（kind:sum 按字典序拼接）
    let usage_hash = sha256_hex(usage.iter().map(|(k, s)| format!("{k}:{s}")).collect::<Vec<_>>().join(";").as_bytes());

    let rate_of = |kind: &str| rates.iter().find(|(k, _, _)| k == kind).cloned();
    let mut lines = Vec::new();
    let mut unrated = Vec::new();
    let mut total = 0i64;
    let mut currency = "CNY".to_string();
    for (kind, sum) in &usage {
        let (price, cur) = match rate_of(kind) {
            Some((_, p, c)) => {
                if !c.is_empty() {
                    currency = c.clone();
                }
                (p, c)
            }
            None => {
                unrated.push(kind.clone());
                (0, currency.clone())
            }
        };
        let amount = sum.saturating_mul(price);
        total = total.saturating_add(amount);
        lines.push(BillLine {
            kind: kind.clone(),
            quantity: *sum,
            unit_price_micros: price,
            amount_micros: amount,
            currency: cur,
        });
    }
    lines.sort_by(|a, b| a.kind.cmp(&b.kind));
    unrated.sort();

    let doc = BillDoc {
        tenant_id: tenant.to_string(),
        period_from: from.to_rfc3339(),
        period_to: to.to_rfc3339(),
        currency,
        usage_hash,
        lines,
        unrated,
        total_micros: total,
    };
    let doc_str = serde_json::to_string(&doc)
        .map_err(|e| forge_core::ForgeError::InvalidState(format!("bill serialize: {e}")))?;
    let doc_hash = sha256_hex(doc_str.as_bytes());

    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO bills (tenant_id, period_from, period_to, doc, doc_hash) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id, period_from, period_to) \
         DO UPDATE SET doc = EXCLUDED.doc, doc_hash = EXCLUDED.doc_hash RETURNING id",
    )
    .bind(tenant)
    .bind(from)
    .bind(to)
    .bind(serde_json::to_value(&doc).map_err(|e| forge_core::ForgeError::InvalidState(format!("bill json: {e}")))?)
    .bind(&doc_hash)
    .fetch_one(pool)
    .await
    .map_err(|e| forge_core::ForgeError::InvalidState(format!("bill upsert: {e}")))?;
    Ok((id, doc_hash, doc))
}

/// 读取账单（doc 从 JSONB 还原为结构体——字段序由结构体决定，导出字节稳定）。
pub async fn get_bill(pool: &sqlx::PgPool, id: i64) -> ForgeResult<Option<BillDoc>> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as("SELECT doc FROM bills WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| forge_core::ForgeError::InvalidState(format!("get_bill: {e}")))?;
    Ok(row.and_then(|(v,)| serde_json::from_value(v).ok()))
}

/// CSV 形态冻结：表头 + 按 kind 字典序的行（unrated 行 unit_price/amount 为 0）。
pub fn bill_to_csv(doc: &BillDoc) -> String {
    let mut out = String::from("kind,quantity,unit_price_micros,amount_micros,currency\n");
    for l in &doc.lines {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            l.kind, l.quantity, l.unit_price_micros, l.amount_micros, l.currency
        ));
    }
    out
}
