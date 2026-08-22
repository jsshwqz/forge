//! Task 存储接口与内存实现。

use crate::model::{AcceptanceCriterion, Task, TaskStatus};
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult, TaskId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 任务存储 trait。
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// 创建新任务。
    async fn create(
        &self,
        goal: String,
        constraints: Vec<String>,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> ForgeResult<Task>;

    /// 获取任务。
    async fn get(&self, id: &TaskId) -> ForgeResult<Task>;

    /// 更新任务状态。
    async fn update_status(&self, id: &TaskId, to: TaskStatus) -> ForgeResult<Task>;

    /// 列出所有任务 ID。
    async fn list(&self) -> ForgeResult<Vec<TaskId>>;
}

/// 内存任务存储。
#[derive(Default)]
pub struct InMemoryTaskStore {
    tasks: Arc<RwLock<HashMap<TaskId, Task>>>,
}

#[async_trait]
impl TaskStore for InMemoryTaskStore {
    async fn create(
        &self,
        goal: String,
        constraints: Vec<String>,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> ForgeResult<Task> {
        let id = TaskId::new_task_id();
        let task = Task::new(id.clone(), goal, constraints, acceptance);
        self.tasks.write().await.insert(id.clone(), task.clone());
        Ok(task)
    }

    async fn get(&self, id: &TaskId) -> ForgeResult<Task> {
        self.tasks
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("task: {}", id)))
    }

    async fn update_status(&self, id: &TaskId, to: TaskStatus) -> ForgeResult<Task> {
        let mut guard = self.tasks.write().await;
        let task = guard
            .get_mut(id)
            .ok_or_else(|| ForgeError::NotFound(format!("task: {}", id)))?;
        task.transition(to)?;
        Ok(task.clone())
    }

    async fn list(&self) -> ForgeResult<Vec<TaskId>> {
        Ok(self.tasks.read().await.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AcceptanceCriterion, CheckSpec};

    fn make_ac(id: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: id.into(),
            description: "test".into(),
            check: CheckSpec::FileExists("output.txt".into()),
        }
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let store = InMemoryTaskStore::default();
        let task = store
            .create("goal".into(), vec![], vec![make_ac("AC-1")])
            .await
            .unwrap();
        let got = store.get(&task.id).await.unwrap();
        assert_eq!(got.goal, "goal");
        assert_eq!(got.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_update_status() {
        let store = InMemoryTaskStore::default();
        let task = store
            .create("goal".into(), vec![], vec![make_ac("AC-1")])
            .await
            .unwrap();
        let updated = store.update_status(&task.id, TaskStatus::Planned).await.unwrap();
        assert_eq!(updated.status, TaskStatus::Planned);
    }

    #[tokio::test]
    async fn test_empty_acceptance_cannot_complete() {
        let store = InMemoryTaskStore::default();
        let task = store.create("goal".into(), vec![], vec![]).await.unwrap();
        store.update_status(&task.id, TaskStatus::Planned).await.unwrap();
        store.update_status(&task.id, TaskStatus::Executing).await.unwrap();
        store.update_status(&task.id, TaskStatus::Verifying).await.unwrap();
        let result = store.update_status(&task.id, TaskStatus::Completed).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_not_found() {
        let store = InMemoryTaskStore::default();
        assert!(store.get(&TaskId::new_task_id()).await.is_err());
    }

    #[tokio::test]
    async fn test_list() {
        let store = InMemoryTaskStore::default();
        store.create("g1".into(), vec![], vec![]).await.unwrap();
        store.create("g2".into(), vec![], vec![]).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 2);
    }
}
