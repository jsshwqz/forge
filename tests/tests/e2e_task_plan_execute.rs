//! M2 里程碑门禁：Task → Plan → Execute 端到端集成测试。
//!
//! 测试剧本：
//! 1. 创建任务（含 1 条 FileExists 验收标准）→ 状态 Pending。
//! 2. SequentialPlanner 产出计划 → 任务置 Planned。
//! 3. build_dag + topo_order 无环，ready_steps 返回第一步。
//! 4. ExecutionEngine 经 EngineDispatcher 驱动 ScriptedAgent 调用 EchoTool → 结果 Success。
//! 5. Session 事件序列经 replay 重建，状态一致。

use async_trait::async_trait;
use forge_agent::{
    Agent, AgentAction, AgentConfig, AgentOutcome, TerminateReason, TurnEngine, TurnInput,
};
use forge_core::{ForgeResult, TaskId};
use forge_dag::{build_dag, ready_steps, topo_order};
use forge_exec::{
    EchoTool, EngineDispatcher, ExecutionEngine, PermissionLevel, PermissionPolicy, PolicyContext,
    ToolRouter,
};
use forge_planner::{Planner, SequentialPlanner};
use forge_session::{InMemorySessionStore, SessionStore};
use forge_task::{AcceptanceCriterion, CheckSpec, InMemoryTaskStore, TaskStatus, TaskStore};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 测试夹具：按脚本返回动作的 Agent。
struct ScriptedAgent {
    config: AgentConfig,
    script: Mutex<VecDeque<AgentAction>>,
}

impl ScriptedAgent {
    fn new(actions: Vec<AgentAction>) -> Self {
        Self {
            config: AgentConfig::default(),
            script: Mutex::new(actions.into()),
        }
    }
}

#[async_trait]
impl Agent for ScriptedAgent {
    fn config(&self) -> &AgentConfig {
        &self.config
    }
    async fn act(&self, _input: &TurnInput) -> ForgeResult<AgentAction> {
        let mut g = self.script.lock().await;
        g.pop_front()
            .ok_or_else(|| forge_core::ForgeError::InvalidState("script exhausted".into()))
    }
}

/// AllowAll 策略夹具。
struct AllowAllPolicy;
impl PermissionPolicy for AllowAllPolicy {
    fn check(&self, _level: PermissionLevel, _ctx: &PolicyContext) -> ForgeResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn e2e_task_plan_execute() {
    // === 步骤 1：创建任务 ===
    let task_store = InMemoryTaskStore::default();
    let task = task_store
        .create(
            "生成 output.txt 文件".into(),
            vec![],
            vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "output.txt 存在".into(),
                check: CheckSpec::FileExists("output.txt".into()),
            }],
        )
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Pending);

    // === 步骤 2：SequentialPlanner 产出计划 ===
    let planner = SequentialPlanner {
        capability: "echo".into(),
    };
    let plan = planner.plan(&task).await.unwrap();
    assert!(!plan.steps.is_empty());

    task_store
        .update_status(&task.id, TaskStatus::Planned)
        .await
        .unwrap();

    // === 步骤 3：build_dag + topo_order + ready_steps ===
    let dag = build_dag(&plan).unwrap();
    let topo = topo_order(&dag).unwrap();
    assert_eq!(topo.len(), plan.steps.len());

    let done = std::collections::HashSet::new();
    let ready = ready_steps(&dag, &done).unwrap();
    assert!(!ready.is_empty(), "at least one step should be ready");

    // === 步骤 4：ExecutionEngine + EngineDispatcher + ScriptedAgent + EchoTool ===
    let router = Arc::new(ToolRouter::new());
    router.register(Box::new(EchoTool::new())).unwrap();

    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let session = session_store.create(TaskId::new_task_id()).await.unwrap();

    let engine = Arc::new(ExecutionEngine::new(
        router,
        Arc::new(AllowAllPolicy),
        session_store.clone(),
        std::time::Duration::from_secs(10),
    ));

    // 用 EngineDispatcher 桥接 TurnEngine 与 ExecutionEngine
    let dispatcher = EngineDispatcher::new(engine.clone(), session.id.clone());

    // ScriptedAgent: 调用 echo 工具 → 完成
    let agent = ScriptedAgent::new(vec![
        AgentAction::CallTool {
            tool: "echo".into(),
            input: serde_json::json!({"msg": "hello"}),
        },
        AgentAction::Finish(AgentOutcome::Success),
    ]);

    let turn_engine = TurnEngine::new(agent, 50);
    let report = turn_engine.run(&session.id, &dispatcher).await.unwrap();

    assert_eq!(report.outcome, AgentOutcome::Success);
    assert_eq!(report.terminated, TerminateReason::Finished);

    // Session 应有 2 条事件（ActionDispatched + ActionResult）
    let s = session_store.get(&session.id).await.unwrap();
    assert_eq!(s.events.len(), 2);

    // === 步骤 5：replay 重建状态 ===
    let replayed_state = forge_session::replay(&s.events).unwrap();
    assert_eq!(replayed_state, forge_session::SessionState::Active);
}
