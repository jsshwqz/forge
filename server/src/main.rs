//! forge-server 启动入口。
//!
//! SEC-001：配置性拒绝（非 loopback 无 key）以退出码 78（EX_CONFIG 惯例）退出；
//! 其余错误退出码 1。

#[tokio::main]
async fn main() {
    if let Err(e) = forge_server::run_from_env().await {
        eprintln!("fatal: {e:#}");
        if forge_server::is_config_rejection(&e) {
            std::process::exit(78);
        }
        std::process::exit(1);
    }
}
