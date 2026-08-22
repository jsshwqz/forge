//! forge-event：类型化事件总线。
//!
//! 为 Verification / Recovery 的联动提供解耦通道。
//! 订阅某 topic 后只收到该 topic 事件；订阅前发布的事件不回放。

pub mod bus;

pub use bus::{Event, EventBus, EventStream, InMemoryEventBus, Topic};
