//! Markdown 视图导出：将 JSON 数据源渲染为人类可读的
//! PROGRESS.md / WORKLOG.md / HANDOFF.md。
//!
//! 数据源是 JSON（progress.json / worklog.json / handoff.json），
//! Markdown 只是导出视图，不作为事实源。

use crate::models::{Handoff, ProgressEntry, RecordKind, WorkRecord};

/// 将进度条目渲染为 Markdown 表。
pub fn render_progress(entries: &[ProgressEntry]) -> String {
    let mut out = String::from(
        "# 任务状态索引（由 forge-worklog 自动生成）\n\n\
         > 单一事实源：progress.json。手动修改本文件会被覆盖。\n\n\
         | 任务 ID | 名称 | 状态 | owner | 最近记录 | 提交 |\n\
         |---|---|---|---|---|---|\n",
    );
    for e in entries {
        out.push_str(&format!(
            "| {} | {} | {} {} | {} | {} | {} |\n",
            e.task_id,
            e.name,
            e.status.symbol(),
            e.status.label(),
            e.owner.as_deref().unwrap_or("-"),
            e.last_record.as_deref().unwrap_or("-"),
            e.commit.as_deref().unwrap_or("-")
        ));
    }
    out
}

/// 将工作日志渲染为 Markdown。
pub fn render_worklog(records: &[WorkRecord]) -> String {
    let mut out = String::from(
        "# WORKLOG · 工作日志（由 forge-worklog 自动生成）\n\n\
         > 事实源：worklog.json。分类：R1~R7。\n\n",
    );
    for r in records {
        out.push_str(&format!(
            "## [{}] {} · {} · {}\n\n",
            r.id,
            r.kind.label(),
            r.date,
            r.title
        ));
        if let Some(task) = &r.task_id {
            out.push_str(&format!("- **任务 ID**：{}\n", task));
        }
        out.push_str(&r.body);
        out.push_str("\n\n---\n\n");
    }
    out
}

/// 将交接快照渲染为 Markdown。
pub fn render_handoff(h: &Handoff) -> String {
    let mut out = String::from("# HANDOFF · 交接快照（由 forge-worklog 自动生成）\n\n");
    out.push_str(&format!("- **更新时间**：{}\n", h.updated_at));
    out.push_str(&format!("- **当前状态**：{}\n\n", h.current_status));

    out.push_str("## 🚧 阻塞项\n\n");
    if h.blockers.is_empty() {
        out.push_str("（无）\n");
    } else {
        for b in &h.blockers {
            out.push_str(&format!("- {}\n", b));
        }
    }

    out.push_str("\n## 🗓️ 下一步\n\n");
    if h.next_tasks.is_empty() {
        out.push_str("（无）\n");
    } else {
        out.push_str(
            "| 优先级 | 任务 ID | 名称 | 前置 | 动作 | 验收 |\n|---|---|---|---|---|---|\n",
        );
        for t in &h.next_tasks {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                t.priority, t.task_id, t.name, t.prerequisites, t.actions, t.acceptance
            ));
        }
    }

    out.push_str("\n## ⚠️ 风险/偏差\n\n");
    if h.risks.is_empty() {
        out.push_str("（无）\n");
    } else {
        for r in &h.risks {
            out.push_str(&format!("- {}\n", r));
        }
    }

    out.push_str("\n## 📁 关键文件\n\n");
    for (path, desc) in &h.files {
        out.push_str(&format!("- `{}`：{}\n", path, desc));
    }

    out.push_str(&format!("\n## 🚀 建议\n\n{}\n", h.advice));
    out
}

/// RecordKind 的稳定显示名（导出用）。
pub fn kind_label(kind: RecordKind) -> &'static str {
    kind.label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NextTask, TaskStatus};
    use std::collections::BTreeMap;

    #[test]
    fn test_render_progress_table() {
        let entries = vec![ProgressEntry {
            task_id: "TASK-001".into(),
            name: "测试任务".into(),
            status: TaskStatus::Completed,
            owner: Some("builder-a".into()),
            last_record: Some("R1-001".into()),
            commit: Some("abc123".into()),
        }];
        let md = render_progress(&entries);
        assert!(md.contains("TASK-001"));
        assert!(md.contains("✅"));
        assert!(md.contains("abc123"));
    }

    #[test]
    fn test_render_worklog() {
        let records = vec![WorkRecord {
            id: "R1-001".into(),
            kind: RecordKind::R1Completed,
            date: "2026-08-22".into(),
            task_id: Some("TASK-001".into()),
            title: "完成".into(),
            body: "详细内容".into(),
        }];
        let md = render_worklog(&records);
        assert!(md.contains("R1-001"));
        assert!(md.contains("完成"));
    }

    #[test]
    fn test_render_handoff() {
        let h = Handoff {
            updated_at: "2026-08-22".into(),
            current_status: "进行中".into(),
            blockers: vec!["cargo 不可用".into()],
            next_tasks: vec![NextTask {
                priority: "P0".into(),
                task_id: "TASK-002".into(),
                name: "下一步".into(),
                prerequisites: "环境恢复".into(),
                actions: "继续".into(),
                acceptance: "通过".into(),
            }],
            risks: vec!["风险1".into()],
            files: BTreeMap::from([("AI_WORKFLOW.md".into(), "规范".into())]),
            advice: "先读规范".into(),
        };
        let md = render_handoff(&h);
        assert!(md.contains("cargo 不可用"));
        assert!(md.contains("TASK-002"));
        assert!(md.contains("先读规范"));
    }
}
