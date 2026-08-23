//! forge-server 启动入口。
//!
//! 端口：环境变量 `FORGE_PORT`（默认 8080）。

use forge_server::app;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("FORGE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let app = app();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    println!("forge-server listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
