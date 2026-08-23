//! forge-server 启动入口。

#[tokio::main]
async fn main() {
    if let Err(e) = forge_server::run_from_env().await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
