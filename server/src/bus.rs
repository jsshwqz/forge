//! V6.0 FED-001：PG LISTEN/NOTIFY 进程间事件总线（build_v60.md AF-BP-V60A 契约）。
//!
//! D1 决议：零新中间件，用 PG LISTEN/NOTIFY（撞性能上限再申报升级）。
//! 连接断开由调用方重连（[`PgBus::listen`] 不做重试策略，R3 文档）。

use forge_core::{ForgeError, ForgeResult};
use sqlx::postgres::PgListener;

/// NOTIFY payload 协议上限 8000 字节（R-15 缓解：超限事件只发 id，按需拉全文）。
pub const MAX_NOTIFY_PAYLOAD: usize = 8000;

/// payload 超限校验（纯函数，离线可测）。
fn payload_ok(payload: &str) -> bool {
    payload.len() <= MAX_NOTIFY_PAYLOAD
}

/// PG 事件总线（零状态，全部经由连接池）。
pub struct PgBus;

impl PgBus {
    /// 投递事件到频道（payload ≤ 8000 字节；超限 → InvalidState）。
    pub async fn notify(pool: &sqlx::PgPool, channel: &str, payload: &str) -> ForgeResult<()> {
        if !payload_ok(payload) {
            return Err(ForgeError::InvalidState(format!(
                "bus notify payload {} bytes exceeds {MAX_NOTIFY_PAYLOAD}",
                payload.len()
            )));
        }
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(channel)
            .bind(payload)
            .execute(pool)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("bus notify: {e}")))?;
        Ok(())
    }

    /// 订阅频道，返回文本流（mpsc）；连接断开时流关闭，由调用方重连。
    pub async fn listen(
        pool: &sqlx::PgPool,
        channel: &str,
    ) -> ForgeResult<tokio::sync::mpsc::Receiver<String>> {
        let mut listener = PgListener::connect_with(pool)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("bus listen connect: {e}")))?;
        listener
            .listen(channel)
            .await
            .map_err(|e| ForgeError::InvalidState(format!("bus listen: {e}")))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);
        tokio::spawn(async move {
            // 连接断开（recv Err）即结束转发，调用方负责重连
            while let Ok(msg) = listener.recv().await {
                if tx.send(msg.payload().to_string()).await.is_err() {
                    break; // 接收端已丢弃，结束转发任务
                }
            }
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_rejects_oversize_payload() {
        // 无需真实 PG：超限校验为纯函数，在连接前发生
        let big = "x".repeat(MAX_NOTIFY_PAYLOAD + 1);
        assert!(!payload_ok(&big));
        assert!(payload_ok(&"x".repeat(MAX_NOTIFY_PAYLOAD)));
    }
}
