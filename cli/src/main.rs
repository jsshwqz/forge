//! forge CLI：Aion Forge 2.0 命令行入口。
//!
//! 技术栈冻结决策（施工包 1.1 节）：CLI 使用 clap。
//!
//! 子命令：
//! - `forge serve`  启动 HTTP 服务（等价 forge-server；FORGE_PORT/FORGE_PG_URL 生效）
//! - `forge version` / 默认  输出版本行

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
    /// 启动 HTTP 服务。
    Serve,
    /// 打印版本信息后退出。
    Version,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve) => {
            if let Err(e) = forge_server::run_from_env().await {
                eprintln!("server error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Version) | None => {
            println!("forge {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
