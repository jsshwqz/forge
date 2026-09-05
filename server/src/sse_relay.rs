//! V6.0 FED-002：跨副本 SSE 事件广播（build_v60.md AF-BP-V60A 契约）。
//!
//! 任意副本产生的事件经 PG LISTEN/NOTIFY（FED-001 [`crate::bus::PgBus`]）投递到
//! 订阅同频道的其它副本，SSE 客户端合并本地流与转发流（按事件 id 去重，本地优先）。
//!
//! 规则：R1 去重键 = 事件 JSON 的 `id` 字段，解析失败的事件只走本地通道；
//! R2 relay 只投递 `forge_events` 频道（tenant 子频道 TODO）；R3 总线断连只
//! warn 并继续本地服务；R4 禁止在 SSE 热路径做同步 DB 写。

use forge_core::{ForgeResult, SessionId, TaskId};
use forge_session::model::{Session, SessionEvent, SessionEventKind};
use forge_session::SessionStore;

/// 合并本地广播流与总线转发电流，按事件 `id` 去重（本地优先）。
///
/// 纯函数式组合，可注入假流做单元测试。输出为到达序（本地/转发交织时按实际
/// 到达先后输出）。
pub fn merge_event_streams(
    mut local: tokio::sync::broadcast::Receiver<String>,
    mut relayed: tokio::sync::mpsc::Receiver<String>,
) -> tokio::sync::mpsc::Receiver<String> {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);
    tokio::spawn(async move {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 统一处理规则（两条流、包括某条流关闭后的残余，全部走同一条路径）：
        // - 有 id：对称去重，先到副本胜出（本地流天然更早，体现"本地优先"）；
        // - 无 id/解析失败：本地照常放行，转发流丢弃（R1）。
        async fn emit_if_new(
            evt: String,
            from_local: bool,
            seen: &mut std::collections::HashSet<String>,
            tx: &tokio::sync::mpsc::Sender<String>,
        ) -> Option<()> {
            let id = serde_json::from_str::<serde_json::Value>(&evt)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from));
            match id {
                Some(id) => {
                    if seen.insert(id) {
                        tx.send(evt).await.ok()?;
                    }
                }
                None => {
                    if from_local {
                        tx.send(evt).await.ok()?;
                    }
                }
            }
            Some(())
        }

        let mut local_open = true;
        let mut relayed_open = true;
        loop {
            tokio::select! {
                item = local.recv(), if local_open => match item {
                    Ok(evt) => {
                        if emit_if_new(evt, true, &mut seen, &tx).await.is_none() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => local_open = false,
                },
                item = relayed.recv(), if relayed_open => match item {
                    Some(evt) => {
                        if emit_if_new(evt, false, &mut seen, &tx).await.is_none() { break; }
                    }
                    None => relayed_open = false,
                },
                else => break,
            }
        }
    });
    rx
}

/// 事件追加挂钩：EventStore.append 成功后调用（失败仅记 warn，不阻断写入）。
pub async fn publish_to_bus(pool: &sqlx::PgPool, event_json: &str) {
    if let Err(e) = crate::bus::PgBus::notify(pool, crate::queue::FORGE_EVENTS_CHANNEL, event_json).await {
        eprintln!("sse_relay: publish_to_bus failed (non-blocking): {e}");
    }
}

/// 事件转发器抽象：真实环境走 PG 总线，测试注入失败/空实现。
#[async_trait::async_trait]
pub trait EventRelay: Send + Sync {
    async fn publish(&self, event_json: &str);
}

/// PG 总线转发器。
pub struct PgRelay {
    pool: sqlx::PgPool,
}

impl PgRelay {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl EventRelay for PgRelay {
    async fn publish(&self, event_json: &str) {
        publish_to_bus(&self.pool, event_json).await;
    }
}

/// 会话存储装饰器：append 成功后把事件转发到跨副本总线（失败不阻断写入，R4）。
pub struct RelaySessionStore {
    inner: std::sync::Arc<dyn SessionStore>,
    relay: std::sync::Arc<dyn EventRelay>,
}

impl RelaySessionStore {
    pub fn new(inner: std::sync::Arc<dyn SessionStore>, relay: std::sync::Arc<dyn EventRelay>) -> Self {
        Self { inner, relay }
    }
}

#[async_trait::async_trait]
impl SessionStore for RelaySessionStore {
    async fn create(&self, task_id: TaskId) -> ForgeResult<Session> {
        self.inner.create(task_id).await
    }

    async fn append(
        &self,
        id: &SessionId,
        kind: SessionEventKind,
        payload: serde_json::Value,
    ) -> ForgeResult<SessionEvent> {
        let event = self.inner.append(id, kind, payload).await?;
        // 稳定去重键：session_id:seq（跨副本一致）
        let event_json = serde_json::json!({
            "id": format!("{}:{}", id.as_ref(), event.seq),
            "kind": format!("{:?}", event.kind),
            "payload": event.payload,
        })
        .to_string();
        self.relay.publish(&event_json).await;
        Ok(event)
    }

    async fn get(&self, id: &SessionId) -> ForgeResult<Session> {
        self.inner.get(id).await
    }

    async fn list(&self) -> ForgeResult<Vec<SessionId>> {
        self.inner.list().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::ForgeError;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 冻结测试：假流注入重复 id，输出恰一份。
    #[tokio::test]
    async fn merge_dedupes_by_event_id() {
        let (btx, brx) = tokio::sync::broadcast::channel::<String>(16);
        let (rtx, rrx) = tokio::sync::mpsc::channel::<String>(16);
        let mut out = merge_event_streams(brx, rrx);

        let local_evt = r#"{"id":"evt_1","at":"t"}"#.to_string();
        btx.send(local_evt.clone()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        rtx.send(local_evt.clone()).await.unwrap(); // 转发流重复同一事件
        rtx.send(r#"{"id":"evt_2"}"#.to_string()).await.unwrap();
        drop(btx);
        drop(rtx);

        let mut got = Vec::new();
        while let Some(evt) = out.recv().await {
            got.push(evt);
        }
        assert_eq!(got.len(), 2, "重复 id 必须去重为一份: {got:?}");
        assert!(got.contains(&local_evt));
        assert!(got.iter().any(|e| e.contains("evt_2")));
    }

    /// 冻结测试：本地与转发交替时按到达序输出。
    #[tokio::test]
    async fn merge_prefers_local_order() {
        let (btx, brx) = tokio::sync::broadcast::channel::<String>(16);
        let (rtx, rrx) = tokio::sync::mpsc::channel::<String>(16);
        let mut out = merge_event_streams(brx, rrx);

        btx.send(r#"{"id":"a"}"#.to_string()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        rtx.send(r#"{"id":"b"}"#.to_string()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        btx.send(r#"{"id":"c"}"#.to_string()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(btx);
        drop(rtx);

        let got: Vec<String> = {
            let mut v = Vec::new();
            while let Some(e) = out.recv().await {
                v.push(e);
            }
            v
        };
        let ids: Vec<&str> = got
            .iter()
            .filter_map(|e| serde_json::from_str::<serde_json::Value>(e).ok())
            .filter_map(|v| v["id"].as_str().map(String::from).map(|s| -> &str { Box::leak(s.into_boxed_str()) }))
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"], "必须保持到达序");
    }

    /// 冻结测试：注入坏 relay（转发必失败），append 流程仍成功不阻断。
    #[tokio::test]
    async fn publish_failure_does_not_block() {
        struct FailingRelay {
            called: AtomicBool,
        }
        #[async_trait::async_trait]
        impl EventRelay for FailingRelay {
            async fn publish(&self, _event_json: &str) {
                self.called.store(true, Ordering::SeqCst);
                // 模拟 notify 失败：publish_to_bus 内部只 warn 不上抛
            }
        }
        struct InnerStore;
        #[async_trait::async_trait]
        impl SessionStore for InnerStore {
            async fn create(&self, task_id: TaskId) -> ForgeResult<Session> {
                Ok(Session { id: SessionId::new_session_id(), task_id, state: forge_session::model::SessionState::Active, events: vec![] })
            }
            async fn append(&self, _id: &SessionId, kind: SessionEventKind, payload: serde_json::Value) -> ForgeResult<SessionEvent> {
                Ok(SessionEvent { seq: 1, at: chrono::Utc::now(), kind, payload })
            }
            async fn get(&self, _id: &SessionId) -> ForgeResult<Session> {
                Err(ForgeError::NotFound("n/a".into()))
            }
            async fn list(&self) -> ForgeResult<Vec<SessionId>> {
                Ok(vec![])
            }
        }

        let relay = std::sync::Arc::new(FailingRelay { called: AtomicBool::new(false) });
        let store = RelaySessionStore::new(
            std::sync::Arc::new(InnerStore),
            relay.clone() as std::sync::Arc<dyn EventRelay>,
        );
        let sid = SessionId::new_session_id();
        let event = store
            .append(&sid, SessionEventKind::TaskReceived, serde_json::json!({"probe": 1}))
            .await
            .expect("转发失败不得阻断 append");
        assert_eq!(event.seq, 1);
        assert!(relay.called.load(Ordering::SeqCst), "转发器应被调用（失败被吞为 warn）");
    }

    /// 冻结测试：无 id 字段的 payload 不进转发通道（只留本地）。
    #[tokio::test]
    async fn malformed_event_stays_local() {
        let (btx, brx) = tokio::sync::broadcast::channel::<String>(16);
        let (rtx, rrx) = tokio::sync::mpsc::channel::<String>(16);
        let mut out = merge_event_streams(brx, rrx);

        // 本地发出一条畸形事件（无 id）——本地通道照常放行
        btx.send("not-json-at-all".to_string()).unwrap();
        // 转发流来一条畸形事件——必须被丢弃
        rtx.send("also-malformed".to_string()).await.unwrap();
        rtx.send(r#"{"id":"ok_evt"}"#.to_string()).await.unwrap();
        drop(btx);
        drop(rtx);

        let got: Vec<String> = {
            let mut v = Vec::new();
            while let Some(e) = out.recv().await {
                v.push(e);
            }
            v
        };
        assert!(
            got.iter().any(|e| e == "not-json-at-all"),
            "畸形事件只走本地通道（本地应保留）: {got:?}"
        );
        assert!(
            !got.iter().any(|e| e == "also-malformed"),
            "转发流的畸形事件必须被丢弃"
        );
        assert!(got.iter().any(|e| e.contains("ok_evt")));
    }
}
