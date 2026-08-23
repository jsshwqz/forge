//! forge-server 启动入口。
//!
//! 端口：环境变量 `FORGE_PORT`（默认 8080）。
//! 存储：设置 `FORGE_PG_URL` 时使用 PostgreSQL 持久化，否则内存实现。

use forge_server::{app_with_state, AppState};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("FORGE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let state = match std::env::var("FORGE_PG_URL") {
        Ok(url) => {
            let pool = forge_storage::connect_and_migrate(&url)
                .await
                .unwrap_or_else(|e| panic!("PostgreSQL 连接失败: {e}"));
            println!("storage: PostgreSQL ({url})");
            AppState::new(
                Arc::new(forge_storage::PgTaskStore::new(pool.clone())),
                Arc::new(forge_storage::PgSessionStore::new(pool)),
            )
        }
        Err(_) => {
            println!("storage: in-memory");
            AppState::in_memory()
        }
    };

    let app = app_with_state(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    println!("forge-server listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
