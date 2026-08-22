//! forge-session：会话模型与事件追加式存储。
//!
//! 会话是整条执行链的状态载体，事件只追加不修改。

pub mod model;
pub mod replay;
pub mod store;

pub use model::{Session, SessionEvent, SessionEventKind, SessionState};
pub use replay::replay;
pub use store::{InMemorySessionStore, SessionStore};
