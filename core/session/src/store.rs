//! Session 存储接口与内存实现。

use crate::model::{Session, SessionEvent, SessionEventKind};
use async_trait::async_trait;
use chrono::Utc;
use forge_core::{ForgeError, ForgeResult, SessionId, TaskId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 会话存储 trait。
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 创建新会话。
    async fn create(&self, task_id: TaskId) -> ForgeResult<Session>;

    /// 追加事件，自动分配 seq。
    async fn append(
        &self,
        id: &SessionId,
        kind: SessionEventKind,
        payload: serde_json::Value,
    ) -> ForgeResult<SessionEvent>;

    /// 获取会话。
    async fn get(&self, id: &SessionId) -> ForgeResult<Session>;

    /// 列出所有会话 ID。
    async fn list(&self) -> ForgeResult<Vec<SessionId>>;
}


/// 基于 `tokio::sync::RwLock<HashMap>` 的内存会话存储。
#[derive(Default)]
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<SessionId, Session>>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create(&self, task_id: TaskId) -> ForgeResult<Session> {
        let id = SessionId::new_session_id();
        let session = Session::new(id.clone(), task_id);
        self.sessions.write().await.insert(id.clone(), session.clone());
        Ok(session)
    }

    async fn append(
        &self,
        id: &SessionId,
        kind: SessionEventKind,
        payload: serde_json::Value,
    ) -> ForgeResult<SessionEvent> {
        let mut guard = self.sessions.write().await;
        let session = guard.get_mut(id).ok_or_else(|| ForgeError::NotFound(format!("session: {}", id)))?;
        session.apply_event_kind(&kind)?;
        let seq = session.events.last().map(|e| e.seq + 1).unwrap_or(1);
        let event = SessionEvent { seq, at: Utc::now(), kind, payload };
        session.events.push(event.clone());
        Ok(event)
    }

    async fn get(&self, id: &SessionId) -> ForgeResult<Session> {
        self.sessions.read().await.get(id).cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("session: {}", id)))
    }

    async fn list(&self) -> ForgeResult<Vec<SessionId>> {
        Ok(self.sessions.read().await.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionState;

    #[tokio::test]
    async fn test_create_and_append() {
        let store = InMemorySessionStore::default();
        let task_id = TaskId::new_task_id();
        let session = store.create(task_id).await.unwrap();
        let e1 = store.append(&session.id, SessionEventKind::TaskReceived, serde_json::json!({})).await.unwrap();
        let e2 = store.append(&session.id, SessionEventKind::PlanCreated, serde_json::json!({})).await.unwrap();
        let e3 = store.append(&session.id, SessionEventKind::ActionDispatched, serde_json::json!({})).await.unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);
        assert_eq!(e3.seq, 3);
        let s = store.get(&session.id).await.unwrap();
        assert_eq!(s.events.len(), 3);
    }

    #[tokio::test]
    async fn test_legal_transitions() {
        let store = InMemorySessionStore::default();
        let task_id = TaskId::new_task_id();
        let session = store.create(task_id).await.unwrap();
        store.append(&session.id, SessionEventKind::Failed, serde_json::json!({})).await.unwrap();
        assert_eq!(store.get(&session.id).await.unwrap().state, SessionState::Failed);
        store.append(&session.id, SessionEventKind::Recovered, serde_json::json!({})).await.unwrap();
        assert_eq!(store.get(&session.id).await.unwrap().state, SessionState::Recovering);
        store.append(&session.id, SessionEventKind::ActionDispatched, serde_json::json!({})).await.unwrap();
        assert_eq!(store.get(&session.id).await.unwrap().state, SessionState::Active);
        store.append(&session.id, SessionEventKind::Completed, serde_json::json!({})).await.unwrap();
        assert_eq!(store.get(&session.id).await.unwrap().state, SessionState::Completed);
    }

    #[tokio::test]
    async fn test_illegal_completed_to_active() {
        let store = InMemorySessionStore::default();
        let session = store.create(TaskId::new_task_id()).await.unwrap();
        store.append(&session.id, SessionEventKind::Completed, serde_json::json!({})).await.unwrap();
        assert!(store.append(&session.id, SessionEventKind::TaskReceived, serde_json::json!({})).await.is_err());
    }

    #[tokio::test]
    async fn test_illegal_recovered_from_active() {
        let store = InMemorySessionStore::default();
        let session = store.create(TaskId::new_task_id()).await.unwrap();
        assert!(store.append(&session.id, SessionEventKind::Recovered, serde_json::json!({})).await.is_err());
    }

    #[tokio::test]
    async fn test_illegal_completed_from_failed() {
        let store = InMemorySessionStore::default();
        let session = store.create(TaskId::new_task_id()).await.unwrap();
        store.append(&session.id, SessionEventKind::Failed, serde_json::json!({})).await.unwrap();
        assert!(store.append(&session.id, SessionEventKind::Completed, serde_json::json!({})).await.is_err());
    }

    #[tokio::test]
    async fn test_concurrent_append_no_duplicate_seq() {
        let store = Arc::new(InMemorySessionStore::default());
        let session = store.create(TaskId::new_task_id()).await.unwrap();
        let session_id = session.id;
        let mut handles = Vec::new();
        for _ in 0..100 {
            let sc = store.clone();
            let sid = session_id.clone();
            handles.push(tokio::spawn(async move {
                sc.append(&sid, SessionEventKind::ActionDispatched, serde_json::json!({})).await
            }));
        }
        let mut seqs = Vec::new();
        for h in handles { seqs.push(h.await.unwrap().unwrap().seq); }
        seqs.sort(); seqs.dedup();
        assert_eq!(seqs.len(), 100);
    }

    #[tokio::test]
    async fn test_not_found() {
        let store = InMemorySessionStore::default();
        assert!(store.get(&SessionId::new_session_id()).await.is_err());
    }

    #[tokio::test]
    async fn test_list() {
        let store = InMemorySessionStore::default();
        store.create(TaskId::new_task_id()).await.unwrap();
        store.create(TaskId::new_task_id()).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 2);
    }
}
