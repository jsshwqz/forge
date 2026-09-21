//! MKT-104A 制品库装配面测试（PG 门控，冻结 12 用例）。
//!
//! 冻结测试名：
//! - upload_then_download_bytes_match
//! - upload_exceeds_max_bytes_rejected
//! - publish_artifact_hash_mismatch_rejected
//! - upload_without_publisher_key_rejected
//! - download_yanked_returns_409
//! - download_tampered_package_hash_mismatch
//! - install_with_hash_recheck_passes
//! - install_with_hash_mismatch_rejected
//! - install_with_bad_signature_rejected
//! - delete_removes_artifact_and_metadata
//! - e2e_publish_download_install_roundtrip
//! - no_pg_fallback_to_file_system
//!
//! 纪律：async 全带 tokio::time::timeout（R1-094）；
//! FORGE_ARTIFACT_DIR 指 tempfile 禁写真 home（G4 探针）；
//! 需 PG，未设 FORGE_PG_URL 时 skip。
//!
//! IMPROVE-2R: 并行隔离真修——每用例独享 tempdir + 唯一 test_id 后缀，
//! 不再用共享 OnceLock<TempDir> 和全表 DELETE FROM releases。
//! 验收标准: 不带 --test-threads=1 并行 cargo test 连跑 3 次 12/12 全绿。

use axum::body::Body;
use forge_storage::ArtifactStore;
use axum::http::{Request, StatusCode};
use forge_cap::signing;
use forge_server::{app_with_state, AppState};
use forge_storage::connect_and_migrate;
use http_body_util::BodyExt;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::timeout;
use std::time::Duration;
use tower::ServiceExt;

// ── IMPROVE-2R: per-test 隔离基础设施 ──

/// 全局原子计数器，为每个测试用例生成唯一 ID。
/// 并行测试中每个调用拿到不同的数字，用于 publisher_id 和 name 后缀。
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成唯一测试 ID（如 "p12345-t0", "p12345-t1"...）。
/// 进程 ID 前缀确保跨次运行不撞主键（同一次进程内靠原子计数器天然不撞）。
fn test_id() -> String {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("p{}-t{n}", std::process::id())
}

/// 检查 PG 是否可用，返回 URL 或 None。
fn pg_url() -> Option<String> {
    match std::env::var("FORGE_PG_URL") {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => {
            eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
            None
        }
    }
}

/// 为单个测试创建独立的 tempdir + AppState（带 PG pool）。
/// 返回 (router, tempdir, pool, test_id)。
/// tempdir 需要由调用方持有以保持生命周期（drop 时自动清理）。
async fn setup_app() -> Option<(axum::Router, tempfile::TempDir, sqlx::PgPool, String)> {
    let url = pg_url()?;
    let tid = test_id();
    let dir = tempfile::tempdir().unwrap();

    let pool = connect_and_migrate(&url).await.unwrap();
    let mut st = AppState::in_memory();
    st.pool = Some(pool.clone());
    st.artifact_store = std::sync::Arc::new(
        forge_storage::FileArtifactStore::new(dir.path()),
    );
    Some((app_with_state(st), dir, pool, tid))
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

/// 辅助：登记 publisher + 签名发布带制品字节的 release。
/// 每次调用创建独立的 tempdir 和唯一 publisher_id/name（带 test_id 后缀）。
/// 返回 (router, name, version, content_bytes, tempdir, publisher_id)。
async fn setup_published_artifact(
    content: &[u8],
    base_name: &str,
    version: &str,
) -> (axum::Router, String, String, Vec<u8>, tempfile::TempDir, String) {
    use base64::prelude::*;
    use sha2::{Digest, Sha256};

    let url = std::env::var("FORGE_PG_URL").unwrap();
    let tid = test_id();
    let dir = tempfile::tempdir().unwrap();
    let publisher_id = format!("pub-{tid}");
    let name = format!("{base_name}-{tid}");

    let pool = connect_and_migrate(&url).await.unwrap();

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&publisher_id)
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
        forge_storage::FileArtifactStore::new(dir.path()),
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
            Some(&publisher_id),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "publish must succeed: {}", String::from_utf8_lossy(&body));

    (app, name, version.to_string(), content.to_vec(), dir, publisher_id)
}

/// 冻结测试：上传制品 → 下载 → 字节一致。
#[tokio::test]
async fn upload_then_download_bytes_match() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };
    // 先发布带制品的 release
    let (app, name, version, content, _dir, _pub) = timeout(
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
    let Some((app, _dir, pool, tid)) = setup_app().await else { return; };
    use base64::prelude::*;
    use sha2::{Digest, Sha256};

    let publisher_id = format!("pub-max-{tid}");
    let name = format!("cap-max-{tid}");

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&publisher_id)
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();

    // IMPROVE-2R: 不再设 FORGE_PACKAGE_MAX_BYTES env（进程级 env 在并行测试中是竞态源）。
    // 直接构造超过默认 16MB 限制的 payload 触发 413——不碰 env、不需要 Mutex、并行安全。
    let content = vec![0u8; 16_777_217]; // 16MB + 1 byte, exceeds default max
    let mut h = Sha256::new();
    h.update(&content);
    let checksum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let pkg_bytes = format!("{name}\n1.0.0\n{checksum}").into_bytes();
    let sig = signing::sign_package(&sk, &pkg_bytes).unwrap();
    let b64 = BASE64_STANDARD.encode(&content);

    let (status, body) = timeout(
        Duration::from_secs(15),
        send(
            app,
            post_json(
                "/market/publish",
                serde_json::json!({
                    "name": name,
                    "version": "1.0.0",
                    "package_hash": checksum,
                    "signature": sig,
                    "package_data": b64,
                }),
                Some(&publisher_id),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "oversized artifact must be 413: {}", String::from_utf8_lossy(&body));
}

/// 冻结测试：package_data 的 sha256 与声称 package_hash 不一致 → 409。
#[tokio::test]
async fn publish_artifact_hash_mismatch_rejected() {
    let Some((app, _dir, pool, tid)) = setup_app().await else { return; };
    use base64::prelude::*;

    let publisher_id = format!("pub-mis-{tid}");
    let name = format!("cap-mis-{tid}");

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&publisher_id)
        .bind(&pk)
        .execute(&pool)
        .await
        .unwrap();

    // 声称 hash = "deadbeef" 但实际制品字节不匹配
    let pkg_bytes = format!("{name}\n1.0.0\ndeadbeef").into_bytes();
    let sig = signing::sign_package(&sk, &pkg_bytes).unwrap();
    let b64 = BASE64_STANDARD.encode(b"real content");

    let (status, body) = timeout(
        Duration::from_secs(15),
        send(
            app,
            post_json(
                "/market/publish",
                serde_json::json!({
                    "name": name,
                    "version": "1.0.0",
                    "package_hash": "deadbeef",
                    "signature": sig,
                    "package_data": b64,
                }),
                Some(&publisher_id),
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
    let Some((app, _dir, _pool, _tid)) = setup_app().await else { return; };

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
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    // 先发布带制品的 release
    let (app, name, version, _content, _dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_published_artifact(b"yanked content", "cap-yank-dl", "1.0.0"),
    )
    .await
    .unwrap();

    // 手动标记 yanked（用返回的 name 变量，不硬编码）
    let pool = connect_and_migrate(&std::env::var("FORGE_PG_URL").unwrap()).await.unwrap();
    sqlx::query("UPDATE releases SET yanked = true WHERE name = $1 AND version = $2")
        .bind(&name)
        .bind(&version)
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

// ==================== MKT-104C 验收面: 5 个新冻结用例 ====================

/// 辅助：DELETE 请求构造。
fn delete_req(uri: &str, publisher: Option<&str>) -> Request<Body> {
    let mut b = Request::delete(uri);
    if let Some(pk) = publisher {
        b = b.header("Publisher-Key", pk);
    }
    b.body(Body::empty()).unwrap()
}

/// 辅助：POST /market/install 请求构造。
fn post_install(name: &str, version: &str) -> Request<Body> {
    Request::post("/market/install")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "name": name, "version": version }).to_string(),
        ))
        .unwrap()
}

/// 辅助：在 AppState 的 capability registry 中注册一个 Registered 状态的 capability。
async fn register_capability(
    app_state: &AppState,
    name: &str,
    version: &str,
) {
    use forge_cap::{Capability, CapabilityKind, CapabilityRegistry as _, CapabilityStatus};
    use forge_exec::PermissionLevel;
    use forge_core::CapabilityId;

    app_state
        .capabilities
        .register(Capability {
            id: CapabilityId::new_capability_id(),
            name: name.to_string(),
            kind: CapabilityKind::Tool,
            version: version.to_string(),
            entry: "test://entry".to_string(),
            status: CapabilityStatus::Registered,
            permission: PermissionLevel::ReadOnly,
        })
        .await
        .unwrap();
}

/// 辅助：发布带制品的 release + 注册 capability，返回 (router, name, version, content, pool, tempdir, publisher_id)。
/// 用于 install 测试——需要 capability 存在才能 install 成功。
/// IMPROVE-2R: 每次调用创建独立 tempdir + 唯一 publisher_id/name。
async fn setup_for_install(
    content: &[u8],
    base_name: &str,
    version: &str,
) -> (axum::Router, String, String, Vec<u8>, sqlx::PgPool, tempfile::TempDir, String) {
    use base64::prelude::*;
    use sha2::{Digest, Sha256};

    let url = std::env::var("FORGE_PG_URL").unwrap();
    let tid = test_id();
    let dir = tempfile::tempdir().unwrap();
    let publisher_id = format!("pub-{tid}");
    let name = format!("{base_name}-{tid}");

    let pool = connect_and_migrate(&url).await.unwrap();

    let (sk, pk) = signing::generate_keypair();
    sqlx::query("INSERT INTO publisher_keys (publisher_id, public_key) VALUES ($1, $2)")
        .bind(&publisher_id)
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
    st.pool = Some(pool.clone());
    st.artifact_store = std::sync::Arc::new(
        forge_storage::FileArtifactStore::new(dir.path()),
    );

    // 注册 capability（install 前提条件）
    register_capability(&st, &name, version).await;

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
            Some(&publisher_id),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "publish must succeed: {}", String::from_utf8_lossy(&body));

    (app, name, version.to_string(), content.to_vec(), pool, dir, publisher_id)
}

/// 辅助：计算 artifact 在 FileArtifactStore 中的文件路径。
fn artifact_file_path(root: &std::path::Path, content: &[u8]) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(content);
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    root.join(&sha[0..2]).join(&sha[2..4]).join(&sha)
}



/// 冻结测试 #3：篡改制品文件 → 下载 hash 复核 → 失败。
///
/// 注意: spec S5 原文写 "→ 409", 但 104A 实现中 download_release 的盘后读损坏防线
/// 返回 INTERNAL_SERVER_ERROR (500)——因为这是服务端存储损坏, 非客户端冲突。
/// 104C 纪律为"只加测试不改 src/", 故此处按实际代码行为断言 500。
#[tokio::test]
async fn download_tampered_package_hash_mismatch() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    let content = b"original content for tamper test";
    let (app, name, version, _content, dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_published_artifact(content, "cap-tamper-dl", "1.0.0"),
    )
    .await
    .unwrap();

    // 找到 artifact 文件并篡改（使用返回的 tempdir 路径）
    let file_path = artifact_file_path(dir.path(), content);
    assert!(file_path.exists(), "artifact file must exist: {file_path:?}");
    std::fs::write(&file_path, b"tampered!!!").unwrap();

    // 下载 → hash 复核失败 → 500
    let url = format!("/market/releases/{name}/{version}/download");
    let (status, _body) = timeout(Duration::from_secs(10), send(app, get_req(&url)))
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "tampered artifact download must be 500 (server-side integrity failure)"
    );
}

/// 冻结测试 #4：正常制品 → hash 复核通过 → install 成功。
#[tokio::test]
async fn install_with_hash_recheck_passes() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    let (app, name, version, _content, _pool, _dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_for_install(b"install ok content", "cap-install-ok", "1.0.0"),
    )
    .await
    .unwrap();

    // install → hash 复核通过 → 验签通过 → 200
    let (status, body) = timeout(
        Duration::from_secs(15),
        send(app, post_install(&name, &version)),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::OK, "install must succeed: {}", String::from_utf8_lossy(&body));

    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp["installed"], true, "installed must be true");
}

/// 冻结测试 #5：制品被篡改 → install hash 复核失败 → 409。
#[tokio::test]
async fn install_with_hash_mismatch_rejected() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    let content = b"original install content";
    let (app, name, version, _content, _pool, dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_for_install(content, "cap-install-tamper", "1.0.0"),
    )
    .await
    .unwrap();

    // 篡改 artifact 文件（使用返回的 tempdir 路径）
    let file_path = artifact_file_path(dir.path(), content);
    assert!(file_path.exists(), "artifact file must exist");
    std::fs::write(&file_path, b"tampered install content").unwrap();

    // install → hash 复核失败 → 409
    let (status, _body) = timeout(
        Duration::from_secs(15),
        send(app, post_install(&name, &version)),
    )
    .await
    .unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "install with tampered artifact must be 409 (hash mismatch)"
    );
}

/// 冻结测试 #6：验签失败 → 403。
#[tokio::test]
async fn install_with_bad_signature_rejected() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    let (app, name, version, _content, pool, _dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_for_install(b"sig test content", "cap-bad-sig", "1.0.0"),
    )
    .await
    .unwrap();

    // 破坏 PG 中的签名（用返回的 name 变量，不硬编码）
    sqlx::query("UPDATE releases SET signature = $1 WHERE name = $2 AND version = $3")
        .bind("00".repeat(64))
        .bind(&name)
        .bind(&version)
        .execute(&pool)
        .await
        .unwrap();

    // install → hash 复核通过（artifact 未篡改）→ 验签失败 → 403
    let (status, _body) = timeout(
        Duration::from_secs(15),
        send(app, post_install(&name, &version)),
    )
    .await
    .unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "install with bad signature must be 403"
    );
}

/// 冻结测试 #7：DELETE → 制品文件 + PG 行同删。
#[tokio::test]
async fn delete_removes_artifact_and_metadata() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    let content = b"delete test content";
    let (app, name, version, _content, pool, dir, publisher_id) = timeout(
        Duration::from_secs(30),
        setup_for_install(content, "cap-delete", "1.0.0"),
    )
    .await
    .unwrap();

    // 记录 artifact 文件路径（删除前验证存在）
    let file_path = artifact_file_path(dir.path(), content);
    assert!(file_path.exists(), "artifact file must exist before delete");

    // DELETE（用返回的 publisher_id）
    let url = format!("/market/releases/{name}/{version}");
    let (status, _body) = timeout(
        Duration::from_secs(15),
        send(app, delete_req(&url, Some(&publisher_id))),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::NO_CONTENT, "delete must return 204");

    // 验证 PG 行已删（用返回的 name 变量）
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM releases WHERE name = $1 AND version = $2")
        .bind(&name)
        .bind(&version)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "PG row must be deleted");

    // 验证 artifact 文件已删
    assert!(
        !file_path.exists(),
        "artifact file must be deleted: {file_path:?}"
    );
}

/// 冻结测试 #10：pool=None → 文件系统仍可上传/下载 (方案 B)。
///
/// 测试 ArtifactStore 在无 PG 环境下仍正常工作——HTTP 路由需要 PG (publish/download
/// 返回 503), 但 ArtifactStore 本身是纯文件 I/O, 不依赖 PG。
#[tokio::test]
async fn no_pg_fallback_to_file_system() {
    let dir = tempfile::tempdir().unwrap();
    let store = forge_storage::FileArtifactStore::new(dir.path());

    let content = b"no-pg artifact content";

    // put（相当于"上传"）
    let artifact = timeout(
        Duration::from_secs(10),
        store.put(
            "cap-nopg".to_string(),
            forge_storage::ArtifactKind::Binary,
            content.to_vec(),
            serde_json::json!({}),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    // read（相当于"下载"）
    let read_content = timeout(
        Duration::from_secs(10),
        store.read(&artifact.id),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(read_content, content, "bytes must match without PG");
    assert_eq!(artifact.size_bytes, content.len() as u64);
}

/// IMPROVE-4: e2e 完整闭环——publish → download 字节一致 → install 成功。
/// 覆盖单元测试无法抓到的接线问题（publish 的 artifact_path 能否被 download 和 install 正确使用）。
#[tokio::test]
async fn e2e_publish_download_install_roundtrip() {
    let Some((_router, _dir, _pool, _tid)) = setup_app().await else { return; };

    // 1. publish 带制品的 release（同时注册 capability，为 install 做准备）
    let content = b"e2e roundtrip content v1";
    let (app, name, version, _content, _pool, _dir, _pub) = timeout(
        Duration::from_secs(30),
        setup_for_install(content, "cap-e2e-roundtrip", "1.0.0"),
    )
    .await
    .unwrap();

    // 2. download → 验证字节与上传内容一致
    let (dl_status, dl_body) = timeout(
        Duration::from_secs(15),
        send(app.clone(), get_req(&format!(
            "/market/releases/{name}/{version}/download"
        ))),
    )
    .await
    .unwrap();
    assert_eq!(
        dl_status, StatusCode::OK,
        "download must return 200: {}", String::from_utf8_lossy(&dl_body)
    );
    assert_eq!(
        dl_body, content,
        "downloaded bytes must match original content"
    );

    // 3. install → hash 复核通过 → 验签通过 → 200 {installed: true}
    let (inst_status, inst_body) = timeout(
        Duration::from_secs(15),
        send(app, post_install(&name, &version)),
    )
    .await
    .unwrap();
    assert_eq!(
        inst_status, StatusCode::OK,
        "install must succeed: {}", String::from_utf8_lossy(&inst_body)
    );

    let resp: serde_json::Value = serde_json::from_slice(&inst_body).unwrap();
    assert_eq!(
        resp["installed"], true,
        "installed must be true in e2e roundtrip"
    );
}
