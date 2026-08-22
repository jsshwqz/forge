//! forge-worklog CLI：多 AI 协作记录管理命令行入口。
//!
//! 用法示例：
//!   forge-worklog task list
//!   forge-worklog task add --id TASK-001 --name "任务名"
//!   forge-worklog task start --id TASK-001 --owner builder-a
//!   forge-worklog task complete --id TASK-001 --commit abc1234
//!   forge-worklog task block --id TASK-001 --reason "cargo 不可用"
//!   forge-worklog log add --kind R1 --task TASK-001 --title "完成" --body "..."
//!   forge-worklog log list [--kind R1]
//!   forge-worklog handoff show
//!
//! 注意：本工具当前处于“待验证”状态，因环境 cargo 不可用尚未编译验证。
//! 环境恢复后需运行 `cargo build/test/clippy` 验证通过方可正式使用。

use clap::{Parser, Subcommand};
use forge_worklog::export::{render_handoff, render_progress, render_worklog};
use forge_worklog::models::{Handoff, ProgressEntry, RecordKind, TaskStatus};
use forge_worklog::store::{ensure_dir, Store, StoreError};
use std::path::PathBuf;

/// 多 AI 协作工作记录管理 CLI。
#[derive(Parser)]
#[command(name = "forge-worklog", version, about = "多 AI 协作工作记录管理")]
struct Cli {
    /// 项目根目录（默认自动探测：当前目录或向上找 AI_WORKFLOW.md）。
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 初始化 JSON 数据文件。
    Init,
    /// 任务状态管理。
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// 工作日志管理。
    Log {
        #[command(subcommand)]
        action: LogAction,
    },
    /// 交接快照。
    Handoff {
        #[command(subcommand)]
        action: HandoffAction,
    },
    /// 从 JSON 导出 Markdown 视图（PROGRESS.md / WORKLOG.md / HANDOFF.md）。
    Export,
}

#[derive(Subcommand)]
enum TaskAction {
    /// 列出所有任务。
    List,
    /// 新增任务。
    Add {
        /// 任务 ID，如 TASK-001。
        #[arg(long)]
        id: String,
        /// 任务名称。
        #[arg(long)]
        name: String,
    },
    /// 认领任务（标记 WIP + owner）。
    Start {
        #[arg(long)]
        id: String,
        #[arg(long)]
        owner: String,
    },
    /// 标记完成。
    Complete {
        #[arg(long)]
        id: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// 标记失败。
    Fail {
        #[arg(long)]
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// 标记阻塞。
    Block {
        #[arg(long)]
        id: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum LogAction {
    /// 追加一条工作记录。
    Add {
        /// 记录分类：R1~R7。
        #[arg(long)]
        kind: String,
        /// 关联任务 ID。
        #[arg(long)]
        task: Option<String>,
        /// 标题。
        #[arg(long)]
        title: String,
        /// 正文（Markdown）。
        #[arg(long)]
        body: String,
    },
    /// 列出工作记录。
    List {
        /// 按分类过滤，如 R1。
        #[arg(long)]
        kind: Option<String>,
    },
}

#[derive(Subcommand)]
enum HandoffAction {
    /// 显示当前交接快照。
    Show,
    /// 更新交接快照。
    Update {
        /// 当前状态概述。
        #[arg(long)]
        status: String,
        /// 阻塞项（可多次）。
        #[arg(long)]
        blocker: Vec<String>,
        /// 建议。
        #[arg(long)]
        advice: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), StoreError> {
    // init 命令允许在空目录初始化，root 默认为当前目录；
    // 其他命令需要自动探测项目根。
    let root = match cli.root {
        Some(r) => r,
        None => {
            if matches!(cli.command, Command::Init) {
                std::env::current_dir()?
            } else {
                detect_root()?
            }
        }
    };
    let store = Store::new(root);

    match cli.command {
        Command::Init => {
            ensure_dir(&store.progress_path())?;
            // 创建空数据文件（若不存在）
            if !store.progress_path().exists() {
                store.save_progress(&[])?;
            }
            if !store.worklog_path().exists() {
                store.save_worklog(&[])?;
            }
            if !store.handoff_path().exists() {
                store.save_handoff(&Handoff::default())?;
            }
            println!("已初始化: {}", store.progress_path().display());
            Ok(())
        }
        Command::Task { action } => run_task(&store, action),
        Command::Log { action } => run_log(&store, action),
        Command::Handoff { action } => run_handoff(&store, action),
        Command::Export => {
            let entries = store.load_progress()?;
            let records = store.load_worklog()?;
            let handoff = store.load_handoff()?;

            std::fs::write(
                store.progress_path().with_file_name("PROGRESS.md"),
                render_progress(&entries),
            )?;
            std::fs::write(
                store.worklog_path().with_file_name("WORKLOG.md"),
                render_worklog(&records),
            )?;
            std::fs::write(
                store.handoff_path().with_file_name("HANDOFF.md"),
                render_handoff(&handoff),
            )?;

            println!("已导出 PROGRESS.md / WORKLOG.md / HANDOFF.md");
            Ok(())
        }
    }
}

fn run_task(store: &Store, action: TaskAction) -> Result<(), StoreError> {
    let mut entries = store.load_progress()?;

    match action {
        TaskAction::List => {
            if entries.is_empty() {
                println!("（暂无任务，使用 task add 添加）");
            }
            for e in &entries {
                println!(
                    "{} {} | {} | owner={} | commit={}",
                    e.status.symbol(),
                    e.task_id,
                    e.name,
                    e.owner.as_deref().unwrap_or("-"),
                    e.commit.as_deref().unwrap_or("-")
                );
            }
            Ok(())
        }
        TaskAction::Add { id, name } => {
            if entries.iter().any(|e| e.task_id == id) {
                return Err(StoreError::Invalid(format!("任务已存在: {}", id)));
            }
            entries.push(ProgressEntry {
                task_id: id,
                name,
                status: TaskStatus::NotStarted,
                owner: None,
                last_record: None,
                commit: None,
            });
            store.save_progress(&entries)
        }
        TaskAction::Start { id, owner } => update_task(store, &mut entries, &id, |e| {
            e.status = TaskStatus::Wip;
            e.owner = Some(owner);
            Ok(())
        }),
        TaskAction::Complete { id, commit } => update_task(store, &mut entries, &id, |e| {
            e.status = TaskStatus::Completed;
            e.commit = commit;
            Ok(())
        }),
        TaskAction::Fail { id, .. } => update_task(store, &mut entries, &id, |e| {
            e.status = TaskStatus::Failed;
            Ok(())
        }),
        TaskAction::Block { id, .. } => update_task(store, &mut entries, &id, |e| {
            e.status = TaskStatus::Blocked;
            Ok(())
        }),
    }
}

fn update_task(
    store: &Store,
    entries: &mut [ProgressEntry],
    id: &str,
    f: impl FnOnce(&mut ProgressEntry) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let entry = entries
        .iter_mut()
        .find(|e| e.task_id == id)
        .ok_or_else(|| StoreError::Invalid(format!("任务不存在: {}", id)))?;
    f(entry)?;
    store.save_progress(entries)
}

fn run_log(store: &Store, action: LogAction) -> Result<(), StoreError> {
    match action {
        LogAction::Add {
            kind,
            task,
            title,
            body,
        } => {
            let kind = parse_kind(&kind)?;
            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let record = store.append_record(kind, &date, task, &title, &body)?;
            println!("已追加: {}", record.id);
            Ok(())
        }
        LogAction::List { kind } => {
            let records = store.load_worklog()?;
            let filter = kind.as_deref().and_then(parse_kind_opt);
            for r in &records {
                if let Some(f) = filter {
                    if r.kind != f {
                        continue;
                    }
                }
                println!(
                    "[{}] {} | {} | task={} | {}",
                    r.kind.code(),
                    r.date,
                    r.title,
                    r.task_id.as_deref().unwrap_or("-"),
                    r.id
                );
            }
            Ok(())
        }
    }
}

fn run_handoff(store: &Store, action: HandoffAction) -> Result<(), StoreError> {
    match action {
        HandoffAction::Show => {
            let h = store.load_handoff()?;
            println!("=== HANDOFF ===");
            println!("更新时间: {}", h.updated_at);
            println!("当前状态: {}", h.current_status);
            println!("阻塞项:");
            for b in &h.blockers {
                println!("  - {}", b);
            }
            println!("下一步:");
            for t in &h.next_tasks {
                println!("  [{}] {} {}", t.priority, t.task_id, t.name);
            }
            println!("风险:");
            for r in &h.risks {
                println!("  - {}", r);
            }
            println!("建议: {}", h.advice);
            Ok(())
        }
        HandoffAction::Update {
            status,
            blocker,
            advice,
        } => {
            let mut h = store.load_handoff()?;
            h.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            h.current_status = status;
            if !blocker.is_empty() {
                h.blockers = blocker;
            }
            if let Some(a) = advice {
                h.advice = a;
            }
            store.save_handoff(&h)
        }
    }
}

fn parse_kind(s: &str) -> Result<RecordKind, StoreError> {
    parse_kind_opt(s)
        .ok_or_else(|| StoreError::Invalid(format!("未知记录分类: {}（应为 R1~R7）", s)))
}

fn parse_kind_opt(s: &str) -> Option<RecordKind> {
    match s.trim().to_ascii_uppercase().as_str() {
        "R1" | "R1COMPLETED" => Some(RecordKind::R1Completed),
        "R2" | "R2FAILED" => Some(RecordKind::R2Failed),
        "R3" | "R3BLOCKED" => Some(RecordKind::R3Blocked),
        "R4" | "R4INCOMPLETE" => Some(RecordKind::R4Incomplete),
        "R5" | "R5NEXTACTIONS" => Some(RecordKind::R5NextActions),
        "R6" | "R6DECISION" => Some(RecordKind::R6Decision),
        "R7" | "R7DEVIATIONRISK" => Some(RecordKind::R7DeviationRisk),
        _ => None,
    }
}

/// 自动探测项目根：从当前目录向上找 AI_WORKFLOW.md 或 progress.json。
fn detect_root() -> Result<PathBuf, StoreError> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("AI_WORKFLOW.md").exists() || dir.join("progress.json").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(StoreError::Invalid(
                "未找到项目根（缺少 AI_WORKFLOW.md 或 progress.json），请用 --root 指定".into(),
            ));
        }
    }
}
