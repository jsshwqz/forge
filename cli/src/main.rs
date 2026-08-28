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
    /// 从失败知识库生成回归建议。
    #[command(name = "knowledge-suggest")]
    KnowledgeSuggest {
        /// 输出文件路径
        #[arg(short, long, default_value = "suggestions.json")]
        out: String,
        /// 建议数量
        #[arg(short, long, default_value_t = 5)]
        top_n: u32,
    },
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
        Some(Commands::KnowledgeSuggest { out, top_n }) => {
            use forge_knowledge::{InMemoryKnowledgeBase, suggest as gen_suggest, write_suggestions};
            let kb = InMemoryKnowledgeBase::default();
            let suggestions = gen_suggest(&kb, top_n).await.unwrap_or_default();
            let path = std::path::Path::new(&out);
            if let Err(e) = write_suggestions(&suggestions, path).await {
                eprintln!("failed to write suggestions: {e}");
                std::process::exit(1);
            }
            println!("wrote {} suggestions to {}", suggestions.len(), out);
        }
        Some(Commands::Version) | None => {
            println!("forge {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
