//! Task 模型定义。

use chrono::{DateTime, Utc};
use forge_core::{ForgeError, TaskId};
use serde::{Deserialize, Serialize};

/// 任务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// 待处理。
    Pending,
    /// 已规划。
    Planned,
    /// 执行中。
    Executing,
    /// 验证中。
    Verifying,
    /// 已完成。
    Completed,
    /// 已失败。
    Failed,
}

/// 验收检查规格。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckSpec {
    /// 执行命令，退出码 0 = 通过。
    Command(String),
    /// 文件包含指定内容。
    FileContains {
        /// 文件路径。
        path: String,
        /// 搜索字符串。
        needle: String,
    },
    /// 文件存在。
    FileExists(String),
}

/// 验收标准。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    /// 标准 ID，如 "AC-1"。
    pub id: String,
    /// 描述。
    pub description: String,
    /// 检查规格。
    pub check: CheckSpec,
}

/// Task 对象。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// 任务 ID。
    pub id: TaskId,
    /// 一句话目标。
    pub goal: String,
    /// 约束条件。
    pub constraints: Vec<String>,
    /// 验收标准列表（允许为空，但空则禁止进入 Completed）。
    pub acceptance: Vec<AcceptanceCriterion>,
    /// 当前状态。
    pub status: TaskStatus,
    /// 创建时间。
    pub created_at: DateTime<Utc>,
}

impl Task {
    /// 创建新任务。
    pub fn new(
        id: TaskId,
        goal: String,
        constraints: Vec<String>,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> Self {
        Self {
            id,
            goal,
            constraints,
            acceptance,
            status: TaskStatus::Pending,
            created_at: Utc::now(),
        }
    }

    /// 尝试状态迁移。
    ///
    /// 合法迁移：
    /// - Pending → Planned
    /// - Planned → Executing
    /// - Executing → Verifying
    /// - Verifying → Completed（验收标准非空）
    /// - 任意非终态 → Failed
    /// - Failed → Verifying（重试入口）
    pub fn transition(&mut self, to: TaskStatus) -> Result<(), ForgeError> {
        let legal = match (self.status, to) {
            (TaskStatus::Pending, TaskStatus::Planned) => true,
            (TaskStatus::Planned, TaskStatus::Executing) => true,
            (TaskStatus::Executing, TaskStatus::Verifying) => true,
            (TaskStatus::Verifying, TaskStatus::Completed) => {
                if self.acceptance.is_empty() {
                    return Err(ForgeError::InvalidState(
                        "cannot complete task with empty acceptance criteria".into(),
                    ));
                }
                true
            }
            (TaskStatus::Pending, TaskStatus::Failed)
            | (TaskStatus::Planned, TaskStatus::Failed)
            | (TaskStatus::Executing, TaskStatus::Failed)
            | (TaskStatus::Verifying, TaskStatus::Failed) => true,
            (TaskStatus::Failed, TaskStatus::Verifying) => true,
            _ => false,
        };

        if !legal {
            return Err(ForgeError::InvalidState(format!(
                "illegal task status transition: {:?} -> {:?}",
                self.status, to
            )));
        }

        self.status = to;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(acceptance: Vec<AcceptanceCriterion>) -> Task {
        Task::new(
            TaskId::new_task_id(),
            "test goal".into(),
            vec![],
            acceptance,
        )
    }

    fn make_ac(id: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: id.into(),
            description: "test".into(),
            check: CheckSpec::FileExists("output.txt".into()),
        }
    }

    #[test]
    fn test_legal_full_path() {
        let mut task = make_task(vec![make_ac("AC-1")]);
        task.transition(TaskStatus::Planned).unwrap();
        task.transition(TaskStatus::Executing).unwrap();
        task.transition(TaskStatus::Verifying).unwrap();
        task.transition(TaskStatus::Completed).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_failed_from_pending() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Failed).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
    }

    #[test]
    fn test_failed_from_executing() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Planned).unwrap();
        task.transition(TaskStatus::Executing).unwrap();
        task.transition(TaskStatus::Failed).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
    }

    #[test]
    fn test_retry_from_failed() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Failed).unwrap();
        task.transition(TaskStatus::Verifying).unwrap();
        assert_eq!(task.status, TaskStatus::Verifying);
    }

    #[test]
    fn test_empty_acceptance_cannot_complete() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Planned).unwrap();
        task.transition(TaskStatus::Executing).unwrap();
        task.transition(TaskStatus::Verifying).unwrap();
        let result = task.transition(TaskStatus::Completed);
        assert!(result.is_err());
    }

    #[test]
    fn test_illegal_pending_to_executing() {
        let mut task = make_task(vec![]);
        assert!(task.transition(TaskStatus::Executing).is_err());
    }

    #[test]
    fn test_illegal_completed_to_anything() {
        let mut task = make_task(vec![make_ac("AC-1")]);
        task.transition(TaskStatus::Planned).unwrap();
        task.transition(TaskStatus::Executing).unwrap();
        task.transition(TaskStatus::Verifying).unwrap();
        task.transition(TaskStatus::Completed).unwrap();
        assert!(task.transition(TaskStatus::Failed).is_err());
        assert!(task.transition(TaskStatus::Pending).is_err());
    }

    #[test]
    fn test_illegal_planned_to_verifying() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Planned).unwrap();
        assert!(task.transition(TaskStatus::Verifying).is_err());
    }

    #[test]
    fn test_illegal_failed_to_planned() {
        let mut task = make_task(vec![]);
        task.transition(TaskStatus::Failed).unwrap();
        assert!(task.transition(TaskStatus::Planned).is_err());
    }

    #[test]
    fn test_illegal_executing_to_completed() {
        let mut task = make_task(vec![make_ac("AC-1")]);
        task.transition(TaskStatus::Planned).unwrap();
        task.transition(TaskStatus::Executing).unwrap();
        assert!(task.transition(TaskStatus::Completed).is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let task = make_task(vec![make_ac("AC-1"), make_ac("AC-2")]);
        let json = serde_json::to_string(&task).unwrap();
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(task.id, back.id);
        assert_eq!(task.goal, back.goal);
        assert_eq!(task.acceptance.len(), back.acceptance.len());
        assert_eq!(task.status, back.status);
    }
}
