//! forge-sdk：对外门面（冻结目录槽位 sdk/）。
//!
//! 一行组装核心栈，屏蔽 crate 拼装细节：
//! ```no_run
//! use forge_sdk::ForgeSdk;
//!
//! # async fn demo() -> forge_core::ForgeResult<()> {
//! // 内存栈（测试/演示）
//! let sdk = ForgeSdk::in_memory();
//! let task = sdk.create_task("目标", vec![], vec![]).await?;
//!
//! // PostgreSQL 栈（FORGE_PG_URL 或显式串）
//! let sdk = ForgeSdk::postgres_from_env().await?;
//! # Ok(())
//! # }
//! ```
//!
//! 范围声明（v0）：仅暴露任务/会话两类高频入口与底层 store 访问器；
//! 执行引擎/验证器编排属后续版本。

use forge_core::{ForgeResult, TaskId};
use forge_session::{InMemorySessionStore, Session, SessionStore};
use forge_task::{AcceptanceCriterion, InMemoryTaskStore, Task, TaskStore};
use std::sync::Arc;

/// Aion Forge SDK 句柄。
#[derive(Clone)]
pub struct ForgeSdk {
    tasks: Arc<dyn TaskStore>,
    sessions: Arc<dyn SessionStore>,
    /// 存储后端描述（"in-memory" / "postgres"）。
    pub backend: &'static str,
}

impl ForgeSdk {
    /// 全内存栈。
    pub fn in_memory() -> Self {
        Self {
            tasks: Arc::new(InMemoryTaskStore::default()),
            sessions: Arc::new(InMemorySessionStore::default()),
            backend: "in-memory",
        }
    }

    /// PostgreSQL 栈（自动执行幂等迁移）。
    pub async fn postgres(url: &str) -> ForgeResult<Self> {
        let pool = forge_storage::connect_and_migrate(url).await?;
        Ok(Self {
            tasks: Arc::new(forge_storage::PgTaskStore::new(pool.clone())),
            sessions: Arc::new(forge_storage::PgSessionStore::new(pool)),
            backend: "postgres",
        })
    }

    /// 从环境变量 `FORGE_PG_URL` 组装；未设置则回退内存栈。
    pub async fn postgres_from_env() -> ForgeResult<Self> {
        match std::env::var("FORGE_PG_URL") {
            Ok(url) => Self::postgres(&url).await,
            Err(_) => Ok(Self::in_memory()),
        }
    }

    /// 创建任务。
    pub async fn create_task(
        &self,
        goal: impl Into<String>,
        constraints: Vec<String>,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> ForgeResult<Task> {
        self.tasks.create(goal.into(), constraints, acceptance).await
    }

    /// 读取任务。
    pub async fn get_task(&self, id: &TaskId) -> ForgeResult<Task> {
        self.tasks.get(id).await
    }

    /// 列举任务 ID。
    pub async fn list_tasks(&self) -> ForgeResult<Vec<TaskId>> {
        self.tasks.list().await
    }

    /// 为任务创建会话。
    pub async fn create_session(&self, task_id: TaskId) -> ForgeResult<Session> {
        self.sessions.create(task_id).await
    }

    /// 任务存储访问器（高级用法）。
    pub fn tasks(&self) -> &dyn TaskStore {
        self.tasks.as_ref()
    }

    /// 会话存储访问器（高级用法）。
    pub fn sessions(&self) -> &dyn SessionStore {
        self.sessions.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_task::TaskStatus;

    #[tokio::test]
    async fn in_memory_flow() {
        let sdk = ForgeSdk::in_memory();
        assert_eq!(sdk.backend, "in-memory");

        let t = sdk.create_task("demo", vec![], vec![]).await.unwrap();
        assert_eq!(t.status, TaskStatus::Pending);

        let got = sdk.get_task(&t.id).await.unwrap();
        assert_eq!(got.goal, "demo");

        let s = sdk.create_session(t.id.clone()).await.unwrap();
        assert_eq!(s.task_id, t.id);

        assert_eq!(sdk.list_tasks().await.unwrap().len(), 1);
    }

    #[cfg(feature = "__never")] // PG 流程由 storage/server 集成测试覆盖，此处保持离线
    #[tokio::test]
    async fn postgres_flow_placeholder() {}
}
