//! Session 模型定义。

use chrono::{DateTime, Utc};
use forge_core::{SessionId, TaskId};
use serde::{Deserialize, Serialize};

/// 会话事件类型。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionEventKind {
    /// 任务已接收。
    TaskReceived,
    /// 计划已创建。
    PlanCreated,
    /// 动作已分发。
    ActionDispatched,
    /// 动作结果。
    ActionResult,
    /// 验证结果。
    VerificationResult,
    /// 失败。
    Failed,
    /// 已恢复。
    Recovered,
    /// 已完成。
    Completed,
}

/// 单条会话事件（追加式，不可修改）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 从 1 开始单调递增的序列号。
    pub seq: u64,
    /// 事件发生时间。
    pub at: DateTime<Utc>,
    /// 事件类型。
    pub kind: SessionEventKind,
    /// 事件负载。
    pub payload: serde_json::Value,
}

/// 会话状态。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionState {
    /// 活跃中。
    Active,
    /// 已完成。
    Completed,
    /// 已失败。
    Failed,
    /// 恢复中。
    Recovering,
}

/// 会话对象。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// 会话 ID。
    pub id: SessionId,
    /// 关联的任务 ID。
    pub task_id: TaskId,
    /// 当前状态。
    pub state: SessionState,
    /// 事件序列（追加式）。
    pub events: Vec<SessionEvent>,
}

impl Session {
    /// 创建新会话。
    pub fn new(id: SessionId, task_id: TaskId) -> Self {
        Self {
            id,
            task_id,
            state: SessionState::Active,
            events: Vec::new(),
        }
    }

    /// 尝试状态迁移，非法迁移返回错误。
    pub fn transition(&mut self, to: SessionState) -> forge_core::ForgeResult<()> {
        let allowed = matches!(
            (self.state, to),
            (SessionState::Active, SessionState::Completed)
                | (SessionState::Active, SessionState::Failed)
                | (SessionState::Failed, SessionState::Recovering)
                | (SessionState::Recovering, SessionState::Active)
        );
        if !allowed {
            return Err(forge_core::ForgeError::InvalidState(format!(
                "illegal session state transition: {:?} -> {:?}",
                self.state, to
            )));
        }
        self.state = to;
        Ok(())
    }

    /// 根据 event kind 应用状态变更。无需迁移时返回 Ok。
    pub(crate) fn apply_event_kind(
        &mut self,
        kind: &SessionEventKind,
    ) -> forge_core::ForgeResult<()> {
        if let Some(target) = target_state_for(kind, self.state) {
            self.transition(target)?;
        }
        Ok(())
    }
}

/// 根据 event kind 与当前状态计算目标状态。
///
/// 返回 `None` 表示无需迁移（事件合法，状态不变）。
/// 返回 `Some(state)` 表示需要迁移——若迁移非法，`transition` 会返回 `InvalidState`。
fn target_state_for(
    kind: &SessionEventKind,
    current: SessionState,
) -> Option<SessionState> {
    match (current, kind) {
        (SessionState::Active, SessionEventKind::Completed) => Some(SessionState::Completed),
        (SessionState::Active, SessionEventKind::Failed) => Some(SessionState::Failed),
        (SessionState::Active, SessionEventKind::Recovered) => Some(SessionState::Recovering),
        (SessionState::Active, _) => None,

        (SessionState::Failed, SessionEventKind::Recovered) => Some(SessionState::Recovering),
        (SessionState::Failed, _) => Some(SessionState::Active),

        (SessionState::Recovering, SessionEventKind::Completed) => Some(SessionState::Completed),
        (SessionState::Recovering, SessionEventKind::Failed) => Some(SessionState::Failed),
        (SessionState::Recovering, SessionEventKind::Recovered) => Some(SessionState::Recovering),
        (SessionState::Recovering, _) => Some(SessionState::Active),

        (SessionState::Completed, _) => Some(SessionState::Active),
    }
}
