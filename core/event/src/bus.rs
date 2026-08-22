//! 事件总线实现。

use async_trait::async_trait;
use chrono::Utc;
use forge_core::{ForgeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 事件主题。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Topic {
    Session,
    Task,
    Execution,
    Verification,
    Recovery,
    Capability,
    Product,
}

/// 事件对象。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// 事件 ID，格式为 `evt_uuidv4`。
    pub id: String,
    /// 事件时间。
    pub at: chrono::DateTime<chrono::Utc>,
    /// 事件主题。
    pub topic: Topic,
    /// 事件负载。
    pub payload: serde_json::Value,
}

impl Event {
    /// 创建新事件。
    pub fn new(topic: Topic, payload: serde_json::Value) -> Self {
        Self {
            id: format!("evt_{}", uuid::Uuid::new_v4()),
            at: Utc::now(),
            topic,
            payload,
        }
    }
}

/// 事件流，封装 `broadcast::Receiver`。
pub struct EventStream {
    rx: broadcast::Receiver<Event>,
}

impl EventStream {
    /// 接收下一条事件。
    pub async fn recv(&mut self) -> ForgeResult<Event> {
        self.rx
            .recv()
            .await
            .map_err(|e| forge_core::ForgeError::InvalidState(format!("event stream closed: {}", e)))
    }
}

/// 事件总线 trait。
#[async_trait]
pub trait EventBus: Send + Sync {
    /// 发布事件。永不阻塞超过 1 秒。
    async fn publish(&self, event: Event) -> ForgeResult<()>;

    /// 订阅指定 topic，返回事件流。
    /// 订阅前发布的事件不回放。
    async fn subscribe(&self, topic: Topic) -> ForgeResult<EventStream>;
}

/// 基于 `tokio::sync::broadcast` 的内存事件总线，容量 >= 1024。
pub struct InMemoryEventBus {
    senders: Arc<tokio::sync::RwLock<HashMap<Topic, broadcast::Sender<Event>>>>,
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryEventBus {
    /// 创建容量为 1024 的总线。
    pub fn new() -> Self {
        Self {
            senders: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl EventBus for InMemoryEventBus {
    async fn publish(&self, event: Event) -> ForgeResult<()> {
        let guard = self.senders.read().await;
        if let Some(sender) = guard.get(&event.topic) {
            // send 是非阻塞的，除非所有 receiver 都 lagged
            let _ = sender.send(event);
        }
        Ok(())
    }

    async fn subscribe(&self, topic: Topic) -> ForgeResult<EventStream> {
        let mut guard = self.senders.write().await;
        let sender = guard.entry(topic).or_insert_with(|| {
            let (tx, _rx) = broadcast::channel(1024);
            tx
        });
        Ok(EventStream {
            rx: sender.subscribe(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_topic_filtering() {
        let bus = InMemoryEventBus::default();

        // 先订阅 Execution
        let mut rx = bus.subscribe(Topic::Execution).await.unwrap();

        // 发布 Execution 和 Verification 各一条
        bus.publish(Event::new(Topic::Execution, serde_json::json!({"n": 1})))
            .await
            .unwrap();
        bus.publish(Event::new(Topic::Verification, serde_json::json!({"n": 2})))
            .await
            .unwrap();

        // 只收到 Execution 事件
        let evt = rx.recv().await.unwrap();
        assert_eq!(evt.topic, Topic::Execution);
        assert_eq!(evt.payload["n"], 1);

        // 不应再收到事件（Verification 被过滤）
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(result.is_err(), "should not receive Verification event");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = InMemoryEventBus::default();

        let mut rx1 = bus.subscribe(Topic::Task).await.unwrap();
        let mut rx2 = bus.subscribe(Topic::Task).await.unwrap();

        bus.publish(Event::new(Topic::Task, serde_json::json!({"msg": "hello"})))
            .await
            .unwrap();

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.payload["msg"], "hello");
        assert_eq!(e2.payload["msg"], "hello");
    }

    #[tokio::test]
    async fn test_no_replay_before_subscribe() {
        let bus = InMemoryEventBus::default();

        // 订阅前发布
        bus.publish(Event::new(Topic::Session, serde_json::json!({"old": true})))
            .await
            .unwrap();

        // 订阅后不应收到之前的事件
        let mut rx = bus.subscribe(Topic::Session).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(result.is_err(), "should not replay events before subscribe");

        // 订阅后发布的应该收到
        bus.publish(Event::new(Topic::Session, serde_json::json!({"new": true})))
            .await
            .unwrap();
        let evt = rx.recv().await.unwrap();
        assert_eq!(evt.payload["new"], true);
    }
}
