//! PH2-001b 集成测试：对真实 MinIO 容器验证对象存储。
//!
//! 运行方式：
//! ```powershell
//! $env:FORGE_MINIO_URL  = "http://localhost:19000"
//! $env:FORGE_MINIO_AK   = "forgeadmin"
//! $env:FORGE_MINIO_SK   = "forgeadmin123"
//! cargo test -p forge-storage
//! ```

use forge_artifact::{ArtifactKind, ArtifactStore};
use forge_storage::{MinioArtifactStore, S3Config};

async fn store_or_skip() -> Option<MinioArtifactStore> {
    let (Ok(url), Ok(ak), Ok(sk)) = (
        std::env::var("FORGE_MINIO_URL"),
        std::env::var("FORGE_MINIO_AK"),
        std::env::var("FORGE_MINIO_SK"),
    ) else {
        eprintln!("[skip] FORGE_MINIO_* 未设置——本测试需真实 MinIO");
        return None;
    };
    let cfg = S3Config {
        endpoint: url,
        bucket: format!("forge-test-{}", chrono::Utc::now().timestamp_millis()),
        access_key: ak,
        secret_key: sk,
    };
    Some(MinioArtifactStore::connect(cfg).await.unwrap())
}

#[tokio::test]
async fn minio_put_read_meta_roundtrip() {
    let Some(store) = store_or_skip().await else { return };

    let content = b"minio roundtrip payload".to_vec();
    let art = store
        .put(
            "roundtrip.bin".into(),
            ArtifactKind::Binary,
            content.clone(),
            serde_json::json!({"case": "rt"}),
        )
        .await
        .unwrap();

    // 读回内容一致
    assert_eq!(store.read(&art.id).await.unwrap(), content);

    // 元数据一致
    let meta = store.get_meta(&art.id).await.unwrap();
    assert_eq!(meta.name, "roundtrip.bin");
    assert_eq!(meta.kind, ArtifactKind::Binary);
    assert_eq!(meta.size_bytes, content.len() as u64);
    assert_eq!(meta.checksum_sha256, art.checksum_sha256);
    assert_eq!(meta.meta["case"], "rt");

    // 不存在 → NotFound
    use forge_core::ArtifactId;
    let ghost = ArtifactId::new_artifact_id();
    let err = store.read(&ghost).await.unwrap_err();
    assert!(err.to_string().contains("not found"));

    // 大一点的负载（>1MB）确保分块传输无碍
    let big = vec![7u8; 1024 * 1024 + 17];
    let big_art = store
        .put("big.bin".into(), ArtifactKind::Other, big.clone(), serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(store.read(&big_art.id).await.unwrap(), big);
}

#[tokio::test]
async fn minio_config_validation() {
    let bad = S3Config {
        endpoint: String::new(),
        bucket: "b".into(),
        access_key: "a".into(),
        secret_key: "s".into(),
    };
    assert!(bad.validate().is_err());
}
