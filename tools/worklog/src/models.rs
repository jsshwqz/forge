//! 领域模型：与多 AI 协作规范一一对应。
//!
//! 记录分类（R1~R7）和任务状态枚举均在此定义，
//! 保证所有 AI 使用同一套 Rust 类型，避免自然语言歧义。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 工作记录分类（规范第 6 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordKind {
    /// R1 成功记录：任务完成，DoD 通过，已提交。
    R1Completed,
    /// R2 失败记录：已尝试但未通过验收。
    R2Failed,
    /// R3 阻塞记录：外部条件导致无法继续。
    R3Blocked,
    /// R4 未完成记录：已开始但中断，下次从此继续。
    R4Incomplete,
    /// R5 下一步计划：下一个任务清单。
    R5NextActions,
    /// R6 决策记录：架构/技术决策及理由。
    R6Decision,
    /// R7 偏差与风险：与规格偏差、未落地冻结项、风险。
    R7DeviationRisk,
}

impl RecordKind {
    /// 返回规范中的类别码字符串。
    pub fn code(self) -> &'static str {
        match self {
            RecordKind::R1Completed => "R1",
            RecordKind::R2Failed => "R2",
            RecordKind::R3Blocked => "R3",
            RecordKind::R4Incomplete => "R4",
            RecordKind::R5NextActions => "R5",
            RecordKind::R6Decision => "R6",
            RecordKind::R7DeviationRisk => "R7",
        }
    }

    /// 返回带 emoji 的显示名。
    pub fn label(self) -> &'static str {
        match self {
            RecordKind::R1Completed => "✅ 成功",
            RecordKind::R2Failed => "❌ 失败",
            RecordKind::R3Blocked => "🚧 阻塞",
            RecordKind::R4Incomplete => "📌 未完成",
            RecordKind::R5NextActions => "🗓️ 计划",
            RecordKind::R6Decision => "⚖️ 决策",
            RecordKind::R7DeviationRisk => "⚠️ 偏差/风险",
        }
    }
}

/// 任务状态（规范第 8 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// ⬜ 未开始
    NotStarted,
    /// 📌 WIP（进行中）
    Wip,
    /// ✅ 完成
    Completed,
    /// ❌ 失败
    Failed,
    /// 🚧 阻塞
    Blocked,
}

impl TaskStatus {
    /// 返回显示符号。
    pub fn symbol(self) -> &'static str {
        match self {
            TaskStatus::NotStarted => "⬜",
            TaskStatus::Wip => "📌",
            TaskStatus::Completed => "✅",
            TaskStatus::Failed => "❌",
            TaskStatus::Blocked => "🚧",
        }
    }

    /// 返回显示名。
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::NotStarted => "未开始",
            TaskStatus::Wip => "WIP",
            TaskStatus::Completed => "完成",
            TaskStatus::Failed => "失败",
            TaskStatus::Blocked => "阻塞",
        }
    }
}

/// 进度条目（PROGRESS 表的一行）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressEntry {
    /// 任务 ID，如 AF-CORE-001。
    pub task_id: String,
    /// 任务名称。
    pub name: String,
    /// 当前状态。
    pub status: TaskStatus,
    /// 认领人（AI 名称），可选。
    pub owner: Option<String>,
    /// 最近工作记录 ID（如 R1-001），可选。
    pub last_record: Option<String>,
    /// 提交 Hash，可选。
    pub commit: Option<String>,
}

/// 下一步计划中的单条任务。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NextTask {
    /// 优先级，如 P0/P1/P2。
    pub priority: String,
    /// 任务 ID。
    pub task_id: String,
    /// 任务名称。
    pub name: String,
    /// 前置条件。
    pub prerequisites: String,
    /// 预估动作。
    pub actions: String,
    /// 验收标准。
    pub acceptance: String,
}

/// 交接快照（HANDOFF）。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Handoff {
    /// 更新时间。
    pub updated_at: String,
    /// 当前状态概述。
    pub current_status: String,
    /// 阻塞项列表。
    pub blockers: Vec<String>,
    /// 下一步任务。
    pub next_tasks: Vec<NextTask>,
    /// 风险/偏差摘要。
    pub risks: Vec<String>,
    /// 关键文件。
    pub files: BTreeMap<String, String>,
    /// 给下一个 AI 的建议。
    pub advice: String,
}

/// 工作记录（WORKLOG 的一条）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkRecord {
    /// 记录 ID，如 R1-001。
    pub id: String,
    /// 记录分类。
    pub kind: RecordKind,
    /// 日期（YYYY-MM-DD）。
    pub date: String,
    /// 关联任务 ID，可选。
    pub task_id: Option<String>,
    /// 标题。
    pub title: String,
    /// 正文（Markdown 格式）。
    pub body: String,
}
