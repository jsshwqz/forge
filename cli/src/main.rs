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
    /// KNW-101: 沙箱复现验证回归建议（只读白名单沙箱，10s 上限）。
    #[command(name = "knowledge-verify")]
    KnowledgeVerify {
        /// 建议文件路径（knowledge-suggest 产物）
        #[arg(long = "in")]
        in_file: String,
        /// 验证报告输出路径
        #[arg(short, long, default_value = "verify_report.json")]
        out: String,
    },
    /// KNW-101: 人工逐条批准一条建议（D3：approve 记入账本，明文钥不入日志）。
    #[command(name = "knowledge-approve")]
    KnowledgeApprove {
        /// 用例哈希（verify 报告中的 case_hash）
        #[arg(long)]
        hash: String,
        /// 模式标识（如 PermissionDenied:write_file）
        #[arg(long)]
        pattern: String,
        /// 批准人
        #[arg(long, default_value = "human")]
        approver: String,
        /// 账本路径
        #[arg(short, long, default_value = "artifacts/knw_approvals.jsonl")]
        ledger: String,
    },
    /// KNW-101: 生成固化分支补丁（PR 等价物；主干 HEAD 不变，合入是人工动作）。
    #[command(name = "knowledge-pr")]
    KnowledgePr {
        /// 仓库根目录
        #[arg(short, long, default_value = ".")]
        repo: String,
        /// 建议文件路径（仅取已 approve 的前 5 条用例）
        #[arg(long = "in")]
        in_file: String,
        /// 补丁导出目录
        #[arg(short, long, default_value = "artifacts/knw_patches")]
        out_dir: String,
        /// 账本路径
        #[arg(short, long, default_value = "artifacts/knw_approvals.jsonl")]
        ledger: String,
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
        Some(Commands::KnowledgeVerify { in_file, out }) => {
            use forge_exec::{EchoTool, ExecutionEngine, PermissionLevel, ToolRouter, WriteFileTool};
            use forge_sandbox::{AllowListPolicy, PolicyChain};
            use forge_session::InMemorySessionStore;
            let raw = std::fs::read_to_string(&in_file).unwrap_or_else(|e| {
                eprintln!("failed to read {in_file}: {e}");
                std::process::exit(1);
            });
            let suggestions: Vec<forge_knowledge::RegressionSuggestion> =
                serde_json::from_str(&raw).unwrap_or_else(|e| {
                    eprintln!("failed to parse suggestions: {e}");
                    std::process::exit(1);
                });
            // 契约冻结的沙箱口径：只读白名单
            let ws = tempfile::tempdir().expect("temp workspace");
            let router = ToolRouter::new();
            router.register(Box::new(EchoTool::new())).unwrap();
            router.register(Box::new(WriteFileTool::new(ws.path()))).unwrap();
            let policy = PolicyChain::new()
                .with(Box::new(AllowListPolicy { allowed: vec![PermissionLevel::ReadOnly] }));
            let engine = ExecutionEngine::new(
                std::sync::Arc::new(router),
                std::sync::Arc::new(policy),
                std::sync::Arc::new(InMemorySessionStore::default()),
                std::time::Duration::from_secs(forge_knowledge::VERIFY_TIMEOUT_SECS),
            );
            let mut reports = Vec::new();
            for s in &suggestions {
                let r = forge_knowledge::verify_suggestion(&engine, s)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("verify failed: {e}");
                        std::process::exit(1);
                    });
                println!("[{}] {} {} ({})", if r.reproduced { "REPRODUCED" } else { "NOT-REPRODUCED" }, r.pattern, &r.case_hash[..12.min(r.case_hash.len())], r.detail);
                reports.push(r);
            }
            let json = serde_json::to_string_pretty(&reports).unwrap();
            if let Some(parent) = std::path::Path::new(&out).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&out, json).unwrap_or_else(|e| {
                eprintln!("failed to write {out}: {e}");
                std::process::exit(1);
            });
            println!("wrote {} verify reports to {}", reports.len(), out);
        }
        Some(Commands::KnowledgeApprove { hash, pattern, approver, ledger }) => {
            let a = forge_knowledge::Approval {
                case_hash: hash.clone(),
                pattern: pattern.clone(),
                approver,
                approved_at: chrono::Utc::now(),
            };
            forge_knowledge::append_approval(std::path::Path::new(&ledger), &a)
                .unwrap_or_else(|e| {
                    eprintln!("failed to append approval: {e}");
                    std::process::exit(1);
                });
            println!("approved {} -> {} (ledger: {})", &hash[..12.min(hash.len())], pattern, ledger);
        }
        Some(Commands::KnowledgePr { repo, in_file, out_dir, ledger }) => {
            let raw = std::fs::read_to_string(&in_file).unwrap_or_else(|e| {
                eprintln!("failed to read {in_file}: {e}");
                std::process::exit(1);
            });
            let all: Vec<forge_knowledge::RegressionSuggestion> =
                serde_json::from_str(&raw).unwrap_or_else(|e| {
                    eprintln!("failed to parse suggestions: {e}");
                    std::process::exit(1);
                });
            let book = forge_knowledge::load_ledger(std::path::Path::new(&ledger))
                .unwrap_or_else(|e| {
                    eprintln!("failed to load ledger: {e}");
                    std::process::exit(1);
                });
            // 只取已 approve 的用例，上限 5（D3）
            let approved: Vec<_> = all
                .into_iter()
                .filter(|s| book.iter().any(|a| a.case_hash == forge_knowledge::case_hash(s)))
                .take(forge_knowledge::MAX_CASES_PER_PR)
                .collect();
            if approved.is_empty() {
                eprintln!("no approved cases in ledger — 先 knowledge-verify + knowledge-approve");
                std::process::exit(1);
            }
            let patch = forge_knowledge::forge_pr(
                std::path::Path::new(&repo),
                &approved,
                &book,
                std::path::Path::new(&out_dir),
            )
            .unwrap_or_else(|e| {
                eprintln!("forge_pr failed: {e}");
                std::process::exit(1);
            });
            println!(
                "branch={} cases={} patch={} (head_before={}) — 合入请人工 review 后 apply/am",
                patch.branch, patch.case_count, patch.patch_path.display(), &patch.head_before[..12]
            );
        }
        Some(Commands::Version) | None => {
            println!("forge {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
