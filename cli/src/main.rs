//! forge CLI：Aion Forge 2.0 命令行入口。
//!
//! 技术栈冻结决策（施工包 1.1 节）：CLI 使用 clap。
//!
//! 行为：
//! - `forge`            → 输出 `forge <版本号>` 后退出 0（保持第一阶段 DoD 兼容）
//! - `forge version`    → 同上
//! - `forge --version`  → clap 自动提供的版本输出
//! - `forge --help`     → 帮助信息

use clap::{Parser, Subcommand};

/// Aion Forge 2.0 —— AI 交付流水线核心。
#[derive(Parser)]
#[command(name = "forge", version, about = "Aion Forge 2.0 —— AI 交付流水线核心", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// 子命令集合。
#[derive(Subcommand)]
enum Commands {
    /// 打印版本信息后退出。
    Version,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        // 显式 version 子命令与无参数默认行为一致：
        // 输出含 "forge" 的版本行后退出 0。
        Some(Commands::Version) | None => {
            println!("forge {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
