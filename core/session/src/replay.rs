//! Session 确定性回放。
//!
//! 从事件序列重建最终状态。纯函数：不读全局状态、不读时钟、不做 IO。
//! 确定性要求：相同输入永远得到相同输出。

use crate::model::{Session, SessionState};
use forge_core::{ForgeError, ForgeResult, SessionId, TaskId};

/// 从事件序列重建最终状态。
///
/// - 初始状态为 `Active`。
/// - 状态迁移规则与 `SessionStore::append` 完全一致。
/// - 事件序列为空 → 返回 `Active`。
/// - 遇到非法迁移立即返回 `InvalidState`，指出出错的 seq。
/// - seq 必须从 1 开始单调递增，乱序返回 `InvalidState`。
pub fn replay(events: &[crate::model::SessionEvent]) -> ForgeResult<SessionState> {
    let mut session = Session::new(SessionId::new_session_id(), TaskId::new_task_id());

    for (i, event) in events.iter().enumerate() {
        let expected_seq = (i + 1) as u64;
        if event.seq != expected_seq {
            return Err(ForgeError::InvalidState(format!(
                "out-of-order seq: expected {}, got {} at position {}",
                expected_seq, event.seq, i
            )));
        }

        session.apply_event_kind(&event.kind).map_err(|e| {
            ForgeError::InvalidState(format!("{} at seq {}", e, event.seq))
        })?;
    }

    Ok(session.state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SessionEvent, SessionEventKind};
    use chrono::Utc;

    fn make_event(seq: u64, kind: SessionEventKind) -> SessionEvent {
        SessionEvent {
            seq,
            at: Utc::now(),
            kind,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn test_empty_sequence() {
        let state = replay(&[]).unwrap();
        assert_eq!(state, SessionState::Active);
    }

    #[test]
    fn test_normal_sequence() {
        let events = vec![
            make_event(1, SessionEventKind::TaskReceived),
            make_event(2, SessionEventKind::PlanCreated),
            make_event(3, SessionEventKind::ActionDispatched),
            make_event(4, SessionEventKind::ActionResult),
            make_event(5, SessionEventKind::VerificationResult),
            make_event(6, SessionEventKind::Completed),
        ];
        let state = replay(&events).unwrap();
        assert_eq!(state, SessionState::Completed);
    }

    #[test]
    fn test_failure_recovery_sequence() {
        let events = vec![
            make_event(1, SessionEventKind::TaskReceived),
            make_event(2, SessionEventKind::PlanCreated),
            make_event(3, SessionEventKind::ActionDispatched),
            make_event(4, SessionEventKind::ActionResult),
            make_event(5, SessionEventKind::Failed),
            make_event(6, SessionEventKind::Recovered),
            make_event(7, SessionEventKind::ActionDispatched),
            make_event(8, SessionEventKind::ActionResult),
            make_event(9, SessionEventKind::VerificationResult),
            make_event(10, SessionEventKind::Completed),
        ];
        let state = replay(&events).unwrap();
        assert_eq!(state, SessionState::Completed);
    }

    #[test]
    fn test_illegal_sequence_completed_then_task_received() {
        let events = vec![
            make_event(1, SessionEventKind::Completed),
            make_event(2, SessionEventKind::TaskReceived),
        ];
        let result = replay(&events);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("2"), "error should contain seq number: {}", err);
    }

    #[test]
    fn test_out_of_order_seq() {
        let events = vec![
            make_event(1, SessionEventKind::TaskReceived),
            make_event(3, SessionEventKind::PlanCreated),
            make_event(2, SessionEventKind::ActionDispatched),
        ];
        let result = replay(&events);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("out-of-order"), "error should mention out-of-order: {}", err);
    }

    #[test]
    fn test_deterministic_same_input_same_output() {
        let events = vec![
            make_event(1, SessionEventKind::TaskReceived),
            make_event(2, SessionEventKind::Failed),
            make_event(3, SessionEventKind::Recovered),
            make_event(4, SessionEventKind::ActionDispatched),
            make_event(5, SessionEventKind::Completed),
        ];
        let state1 = replay(&events).unwrap();
        let state2 = replay(&events).unwrap();
        assert_eq!(state1, state2);
    }

    #[test]
    fn test_recovered_from_active_is_illegal() {
        let events = vec![
            make_event(1, SessionEventKind::Recovered),
        ];
        assert!(replay(&events).is_err());
    }
}
