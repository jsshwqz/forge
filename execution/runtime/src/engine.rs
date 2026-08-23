//! 执行运行时引擎。

use crate::policy::{PermissionPolicy, PolicyContext};
use crate::router::ToolRouter;
use chrono::Utc;
use forge_agent::AgentRole;
use forge_core::{ArtifactId, ExecutionId, ForgeResult, SessionId};
use forge_session::{SessionEventKind, SessionStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// 执行请求。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// 执行 ID。
    pub execution_id: ExecutionId,
    /// 会话 ID。
    pub session_id: SessionId,
    /// 步骤 ID（可空字符串表示非计划内调用）。
    pub step_id: String,
    /// 工具名称。
    pub tool: String,
    /// 工具输入。
    pub input: serde_json::Value,
}

/// 执行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Success,
    Failed,
    Timeout,
    PermissionDenied,
}

/// 执行结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// 执行 ID。
    pub execution_id: ExecutionId,
    /// 执行状态。
    pub status: ExecutionStatus,
    /// 输出（失败时为 {"error": "..."}）。
    pub output: serde_json::Value,
    /// 关联的产物 ID 列表（第一阶段可为空）。
    pub artifact_ids: Vec<ArtifactId>,
    /// 开始时间。
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// 结束时间。
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

/// 执行引擎。
pub struct ExecutionEngine {
    router: Arc<ToolRouter>,
    policy: Arc<dyn PermissionPolicy>,
    session_store: Arc<dyn SessionStore>,
    timeout: Duration,
}

impl ExecutionEngine {
    /// 创建执行引擎。
    pub fn new(
        router: Arc<ToolRouter>,
        policy: Arc<dyn PermissionPolicy>,
        session_store: Arc<dyn SessionStore>,
        timeout: Duration,
    ) -> Self {
        Self {
            router,
            policy,
            session_store,
            timeout,
        }
    }

    /// 执行请求。
    ///
    /// - 权限拒绝 → status=PermissionDenied
    /// - 超时 → Timeout
    /// - 工具报错 → Failed
    /// - 本方法自身不返回 Err（除内部存储故障外），工具级失败体现在 status 中。
    pub async fn execute(&self, req: ExecutionRequest) -> ForgeResult<ExecutionResult> {
        let started_at = Utc::now();

        // 追加 ActionDispatched 事件
        let _ = self
            .session_store
            .append(
                &req.session_id,
                SessionEventKind::ActionDispatched,
                serde_json::json!({
                    "tool": req.tool,
                    "step_id": req.step_id,
                }),
            )
            .await;
        info!(
            execution_id = %req.execution_id,
            tool = %req.tool,
            step_id = %req.step_id,
            session_id = %req.session_id,
            "execution started"
        );

        // 路由工具
        let tool = match self.router.route(&req.tool) {
            Ok(t) => t,
            Err(e) => {
                warn!(execution_id = %req.execution_id, tool = %req.tool, error = %e, "tool routing failed");
                let finished_at = Utc::now();
                let result = ExecutionResult {
                    execution_id: req.execution_id,
                    status: ExecutionStatus::Failed,
                    output: serde_json::json!({"error": e.to_string()}),
                    artifact_ids: vec![],
                    started_at,
                    finished_at,
                };
                self.record_result(&req.session_id, &result).await;
                return Ok(result);
            }
        };

        // 权限检查
        let ctx = PolicyContext {
            session_id: req.session_id.clone(),
            tool_name: req.tool.clone(),
            requester_role: AgentRole::Builder,
        };

        if let Err(e) = self.policy.check(tool.descriptor().permission, &ctx) {
            let reason = e.to_string();
            warn!(
                execution_id = %req.execution_id,
                tool = %req.tool,
                required_level = ?tool.descriptor().permission,
                reason = %reason,
                "permission denied"
            );
            let finished_at = Utc::now();
            let result = ExecutionResult {
                execution_id: req.execution_id,
                status: ExecutionStatus::PermissionDenied,
                output: serde_json::json!({"error": reason}),
                artifact_ids: vec![],
                started_at,
                finished_at,
            };
            self.record_result(&req.session_id, &result).await;
            return Ok(result);
        }

        // 超时受控调用
        let invoke_result = tokio::time::timeout(self.timeout, tool.invoke(req.input.clone())).await;

        let (status, output) = match invoke_result {
            Ok(Ok(value)) => (ExecutionStatus::Success, value),
            Ok(Err(e)) => (
                ExecutionStatus::Failed,
                serde_json::json!({"error": e.to_string()}),
            ),
            Err(_) => (
                ExecutionStatus::Timeout,
                serde_json::json!({"error": "execution timed out"}),
            ),
        };

        let finished_at = Utc::now();
        info!(
            execution_id = %req.execution_id,
            tool = %req.tool,
            ?status,
            duration_ms = (finished_at - started_at).num_milliseconds(),
            "execution finished"
        );
        let result = ExecutionResult {
            execution_id: req.execution_id,
            status,
            output,
            artifact_ids: vec![],
            started_at,
            finished_at,
        };

        self.record_result(&req.session_id, &result).await;
        Ok(result)
    }

    async fn record_result(&self, session_id: &SessionId, result: &ExecutionResult) {
        let _ = self
            .session_store
            .append(
                session_id,
                SessionEventKind::ActionResult,
                serde_json::json!({
                    "status": format!("{:?}", result.status),
                    "execution_id": result.execution_id.to_string(),
                }),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission_level::PermissionLevel;
    use crate::policy::{PermissionPolicy, PolicyContext};
    use crate::router::{EchoTool, ToolRouter};
    use forge_core::{ExecutionId, ForgeError, SessionId, TaskId};
    use forge_session::InMemorySessionStore;
    use std::collections::HashMap;
    use std::sync::RwLock;

    /// 测试夹具：允许所有权限。
    struct AllowAllPolicy;
    impl PermissionPolicy for AllowAllPolicy {
        fn check(&self, _level: PermissionLevel, _ctx: &PolicyContext) -> ForgeResult<()> {
            Ok(())
        }
    }

    /// 测试夹具：只读策略。
    struct ReadOnlyPolicy;
    impl PermissionPolicy for ReadOnlyPolicy {
        fn check(&self, level: PermissionLevel, _ctx: &PolicyContext) -> ForgeResult<()> {
            if level == PermissionLevel::ReadOnly {
                Ok(())
            } else {
                Err(ForgeError::PermissionDenied("read-only".into()))
            }
        }
    }

    /// 测试夹具：睡眠工具。
    struct SleepTool {
        duration: Duration,
        descriptor: crate::router::ToolDescriptor,
    }
    #[async_trait::async_trait]
    impl crate::router::Tool for SleepTool {
        fn descriptor(&self) -> &crate::router::ToolDescriptor {
            &self.descriptor
        }
        async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
            tokio::time::sleep(self.duration).await;
            Ok(serde_json::json!({"slept": true}))
        }
    }

    fn make_request(session_id: &SessionId, tool: &str) -> ExecutionRequest {
        ExecutionRequest {
            execution_id: ExecutionId::new_execution_id(),
            session_id: session_id.clone(),
            step_id: "step_1".into(),
            tool: tool.into(),
            input: serde_json::json!({"msg": "hello"}),
        }
    }

    #[tokio::test]
    async fn test_echo_success() {
        let router = Arc::new(ToolRouter::new());
        router.register(Box::new(EchoTool::new())).unwrap();
        let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let session = session_store.create(TaskId::new_task_id()).await.unwrap();

        let engine = ExecutionEngine::new(
            router,
            Arc::new(AllowAllPolicy),
            session_store.clone(),
            Duration::from_secs(5),
        );

        let req = make_request(&session.id, "echo");
        let result = engine.execute(req).await.unwrap();

        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.output["echo"]["msg"], "hello");

        // Session 应有 2 条事件
        let s = session_store.get(&session.id).await.unwrap();
        assert_eq!(s.events.len(), 2);
    }

    #[tokio::test]
    async fn test_timeout() {
        let router = Arc::new(ToolRouter::new());
        let sleep_tool = SleepTool {
            duration: Duration::from_secs(2),
            descriptor: crate::router::ToolDescriptor {
                name: "sleep".into(),
                description: "sleeps".into(),
                input_schema: serde_json::json!({}),
                permission: PermissionLevel::ReadOnly,
            },
        };
        router.register(Box::new(sleep_tool)).unwrap();

        let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let session = session_store.create(TaskId::new_task_id()).await.unwrap();

        let engine = ExecutionEngine::new(
            router,
            Arc::new(AllowAllPolicy),
            session_store,
            Duration::from_millis(100),
        );

        let req = make_request(&session.id, "sleep");
        let result = engine.execute(req).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Timeout);
    }

    #[tokio::test]
    async fn test_permission_denied() {
        let router = Arc::new(ToolRouter::new());
        // 注册一个 WorkspaceWrite 级别的工具
        struct WriteTool;
        #[async_trait::async_trait]
        impl crate::router::Tool for WriteTool {
            fn descriptor(&self) -> &crate::router::ToolDescriptor {
                use std::sync::OnceLock;
                static DESC: OnceLock<crate::router::ToolDescriptor> = OnceLock::new();
                DESC.get_or_init(|| crate::router::ToolDescriptor {
                    name: "write".into(),
                    description: "writes".into(),
                    input_schema: serde_json::json!({}),
                    permission: PermissionLevel::WorkspaceWrite,
                })
            }
            async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
                Ok(input)
            }
        }
        router.register(Box::new(WriteTool)).unwrap();

        let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let session = session_store.create(TaskId::new_task_id()).await.unwrap();

        let engine = ExecutionEngine::new(
            router,
            Arc::new(ReadOnlyPolicy),
            session_store,
            Duration::from_secs(5),
        );

        let req = make_request(&session.id, "write");
        let result = engine.execute(req).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::PermissionDenied);
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let router = Arc::new(ToolRouter::new());
        let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let session = session_store.create(TaskId::new_task_id()).await.unwrap();

        let engine = ExecutionEngine::new(
            router,
            Arc::new(AllowAllPolicy),
            session_store,
            Duration::from_secs(5),
        );

        let req = make_request(&session.id, "nonexistent");
        let result = engine.execute(req).await.unwrap();
        assert_eq!(result.status, ExecutionStatus::Failed);
    }
}
