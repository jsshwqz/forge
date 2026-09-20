//! MKT-104A 制品库装配面测试（PG 门控，冻结 5 用例）。
//!
//! 冻结测试名：
//! - upload_then_download_bytes_match
//! - upload_exceeds_max_bytes_rejected
//! - publish_artifact_hash_mismatch_rejected
//! - upload_without_publisher_key_rejected
//! - download_yanked_returns_409
//!
//! 纪律：async 全带 tokio::time::timeout（R1-094）；
//! FORGE_ARTIFACT_DIR 指 tempfile 禁写真 home（G4 探针）；
//! 需 PG，未设 FORGE_PG_URL 时 skip。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_cap::signing;
use forge_server::{app_with_state, AppState};
use forge_storage::connect_and_migrate;
use http_body_util::BodyExt;
use std::sync::OnceLock;
use tokio::time::timeout;
use std::time::Duration;
use tower::ServiceExt;

/// tempfile 共享目录（同一测试进程内多个用例可共用，最后由 tempdir 析构清理）。
static ARTIFACT_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

fn artifact_dir() -> &'static tempfile::TempDir {
    ARTIFACT_DIR.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        std::env::set_var("FORGE_ARTIFACT_DIR", d.path());
        d
    })
}

async fn app() -> Option<axum::Router> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    // 初始化 tempfile artifact dir（保证在 app 构造前设好 env）
    let _ = artifact_dir();

    let pool = connect_and_migrate(&url).await.unwrap();
    // 测试隔离：清空 release/publisher 残留
    sqlx::query("DELETE FROM releases").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM publisher_keys").execute(&pool).await.unwrap();

    let mut st = AppState::in_memory();
    st.pool = Some(pool);
    // 覆盖 artifact_store 为指向 tempfile 的 FileArtifactStore
    st.artifact_store = std::sync::Arc::new(
        forge_storage::FileArtifactStore::new(artifact_dir().path()),
    );
    Some(app_with_state(st))
}

async fn send(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, Vec<u8>) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

fn post_json(uri: &str, body: serde_json::Value, publisher: Option<&str>) -> Request<Body> {
    let mut b = Request::post(uri).header("content-type", "application/json");
    if let Some(pk) = publisher {
        b = b.header("Publisher-Key", pk);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn get_req(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

/// 辅助：登记 publisher + 签名发布带制品字节的 release，返回 (router, name, version, content_bytes)。
async fn setup_published_artifact(
    content: &[u8],
    name: &str,
    version: &str,
) -> (axum::Router, String, String, Vec<u8>) {
    use base64::prelude::*;
    use sha2::{Digest, Sha256};

    let pool = connect_and_migrate(&std::env::var("FORGE_PG_URL").unwrap()).await.unwrap();
    sqlx::query("DELETE FROM releases").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM publisher_keys").execute(&pool).await.unwrap();

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ('pub-art', $1)")
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();

    let mut h = Sha256::new();
    h.update(content);
    let checksum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let pkg_bytes = format!("{name}\n{version}\n{checksum}").into_bytes();
    let sig = signing::sign_package(&sk, &pkg_bytes).unwrap();
    let b64 = BASE64_STANDARD.encode(content);

    let mut st = AppState::in_memory();
    st.pool = Some(pool);
    st.artifact_store = std::sync::Arc::new(
        forge_storage::FileArtifactStore::new(artifact_dir().path()),
    );
    let app = app_with_state(st);

    let (status, body) = send(
        app.clone(),
        post_json(
            "/market/publish",
            serde_json::json!({
                "name": name,
                "version": version,
                "package_hash": checksum,
                "signature": sig,
                "package_data": b64,
            }),
            Some("pub-art"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "publish must succeed: {}", String::from_utf8_lossy(&body));

    (app, name.to_string(), version.to_string(), content.to_vec())
}

/// 冻结测试：上传制品 → 下载 → 字节一致。
#[tokio::test]
async fn upload_then_download_bytes_match() {
    let Some(_app) = app().await else { return; };
    // 先发布带制品的 release
    let (app, name, version, content) = timeout(
        Duration::from_secs(30),
        setup_published_artifact(b"artifact content v1", "cap-art", "1.0.0"),
    )
    .await
    .unwrap();

    // 下载
    let url = format!("/market/releases/{name}/{version}/download");
    let (status, body) = timeout(Duration::from_secs(10), send(app, get_req(&url)))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "download must return 200");
    assert_eq!(body, content, "downloaded bytes must match uploaded");
}

/// 冻结测试：超过 FORGE_PACKAGE_MAX_BYTES → 413。
#[tokio::test]
async fn upload_exceeds_max_bytes_rejected() {
    let Some(app) = app().await else { return; };
    use base64::prelude::*;
    use sha2::{Digest, Sha256};

    let pool = connect_and_migrate(&std::env::var("FORGE_PG_URL").unwrap()).await.unwrap();
    sqlx::query("DELETE FROM releases").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM publisher_keys").execute(&pool).await.unwrap();

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ('pub-max', $1)")
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();

    // 设置很小的 limit
    std::env::set_var("FORGE_PACKAGE_MAX_BYTES", "100");

    let content = vec![0u8; 200]; // 200 bytes, exceeds limit
    let mut h = Sha256::new();
    h.update(&content);
    let checksum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let pkg_bytes = format!("cap-max\n1.0.0\n{checksum}").into_bytes();
    let sig = signing::sign_package(&sk, &pkg_bytes).unwrap();
    let b64 = BASE64_STANDARD.encode(&content);

    let (status, body) = timeout(
        Duration::from_secs(15),
        send(
            app,
            post_json(
                "/market/publish",
                serde_json::json!({
                    "name": "cap-max",
                    "version": "1.0.0",
                    "package_hash": checksum,
                    "signature": sig,
                    "package_data": b64,
                }),
                Some("pub-max"),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "oversized artifact must be 413: {}", String::from_utf8_lossy(&body));

    // 恢复默认
    std::env::remove_var("FORGE_PACKAGE_MAX_BYTES");
}

/// 冻结测试：package_data 的 sha256 与声称 package_hash 不一致 → 409。
#[tokio::test]
async fn publish_artifact_hash_mismatch_rejected() {
    let Some(app) = app().await else { return; };
    use base64::prelude::*;

    let pool = connect_and_migrate(&std::env::var("FORGE_PG_URL").unwrap()).await.unwrap();
    sqlx::query("DELETE FROM releases").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM publisher_keys").execute(&pool).await.unwrap();

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ('pub-mis', $1)")
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();

    // 声称 hash = "deadbeef" 但实际制品字节不匹配
    let pkg_bytes = "cap-mis\n1.0.0\ndeadbeef".to_string().into_bytes();
    let sig = signing::sign_package(&sk, &pkg_bytes).unwrap();
    let b64 = BASE64_STANDARD.encode(b"real content");

    let (status, body) = timeout(
        Duration::from_secs(15),
        send(
            app,
            post_json(
                "/market/publish",
                serde_json::json!({
                    "name": "cap-mis",
                    "version": "1.0.0",
                    "package_hash": "deadbeef",
                    "signature": sig,
                    "package_data": b64,
                }),
                Some("pub-mis"),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "hash mismatch must be 409: {}", String::from_utf8_lossy(&body));
}

/// 冻结测试：缺 Publisher-Key → 403。
#[tokio::test]
async fn upload_without_publisher_key_rejected() {
    let Some(app) = app().await else { return; };

    let (status, body) = timeout(
        Duration::from_secs(10),
        send(
            app,
            post_json(
                "/market/publish",
                serde_json::json!({
                    "name": "cap-nopub",
                    "version": "1.0.0",
                    "package_hash": "abc123",
                    "signature": "00".repeat(64),
                    "package_data": "dGVzdA==",
                }),
                None,
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "missing Publisher-Key must be 403: {}", String::from_utf8_lossy(&body));
}

/// 冻结测试：yanked release 下载 → 409。
#[tokio::test]
async fn download_yanked_returns_409() {
    let Some(_app) = app().await else { return; };

    // 先发布带制品的 release
    let (app, name, version, _content) = timeout(
        Duration::from_secs(30),
        setup_published_artifact(b"yanked content", "cap-yank-dl", "1.0.0"),
    )
    .await
    .unwrap();

    // 手动标记 yanked
    let pool = connect_and_migrate(&std::env::var("FORGE_PG_URL").unwrap()).await.unwrap();
    sqlx::query("UPDATE releases SET yanked = true WHERE name = 'cap-yank-dl' AND version = '1.0.0'")
        .execute(&pool)
        .await
        .unwrap();

    // 下载 → 409
    let url = format!("/market/releases/{name}/{version}/download");
    let (status, _body) = timeout(Duration::from_secs(10), send(app, get_req(&url)))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "yanked release download must be 409");
}
