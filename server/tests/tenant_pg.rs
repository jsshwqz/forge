//! V7 TEN-004：租户态持久化（build_v70b.md 冻结测试，PG 门控，照抄 queue_pg.rs 门控写法）。
//!
//! 冻结测试名：
//! - pg_key_store_persists_across_pools（写入 → 新池模拟重启 → 命中同租户）
//! - pg_quota_store_persists_across_pools
//! - issue_hash_sha256_roundtrip（issue 返回明文，sha256_hex(raw) 直接命中）
//! - pg_store_unavailable_maps_503（坏 URL 池 → 前缀错误 + 纯函数判定）
//! - pg_unknown_tenant_rejected（未登记租户 → InvalidState，不静默）

use forge_core::ForgeError;
use forge_server::auth::{is_pg_store_unavailable, sha256_hex, PgTenantKeyStore, TenantKeyStore as _};
use forge_server::quota::{PgQuotaStore, QuotaStore as _, QuotaView};
use forge_storage::connect_and_migrate;

async fn pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return None;
    };
    Some(connect_and_migrate(&url).await.unwrap())
}

/// 测试租户唯一名（避免并行互踩）。
fn probe_tenant() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("t_probe_{nanos}")
}

/// 冻结测试：issue → 丢弃原池 → 新池（模拟重启）→ tenant_of(sha256_hex(raw)) 命中同租户。
#[tokio::test]
async fn pg_key_store_persists_across_pools() {
    let Some(url) = std::env::var("FORGE_PG_URL").ok() else { return };
    let tenant = probe_tenant();
    // 前置：登记租户（tenants 表 FK）
    {
        let pool = connect_and_migrate(&url).await.unwrap();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'probe') ON CONFLICT (id) DO NOTHING")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
    }
    let raw = {
        let pool = connect_and_migrate(&url).await.unwrap();
        PgTenantKeyStore::new(pool).issue(&tenant).await.unwrap()
    }; // 原池在此 drop —— 模拟进程退出

    let got = {
        let pool = connect_and_migrate(&url).await.unwrap(); // 全新连接池
        PgTenantKeyStore::new(pool).tenant_of(&sha256_hex(raw.as_bytes())).await.unwrap()
    };
    assert_eq!(got.as_deref(), Some(tenant.as_str()), "重启后必须命中同租户");
}

/// 冻结测试：set → 新池 → of 数值一致（模拟重启）。
#[tokio::test]
async fn pg_quota_store_persists_across_pools() {
    let Some(url) = std::env::var("FORGE_PG_URL").ok() else { return };
    let tenant = probe_tenant();
    {
        let pool = connect_and_migrate(&url).await.unwrap();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'probe') ON CONFLICT (id) DO NOTHING")
            .bind(&tenant)
            .execute(&pool)
            .await
            .unwrap();
    }
    {
        let pool = connect_and_migrate(&url).await.unwrap();
        PgQuotaStore::new(pool)
            .set(&tenant, QuotaView { max_concurrent: 7, daily_tasks: 77 })
            .await
            .unwrap();
    }
    let got = {
        let pool = connect_and_migrate(&url).await.unwrap();
        PgQuotaStore::new(pool).of(&tenant).await.unwrap()
    };
    assert_eq!(got.max_concurrent, 7);
    assert_eq!(got.daily_tasks, 77);
}

/// 冻结测试：issue 返回明文 raw，sha256_hex(raw) 直接命中（哈希口径冻结）。
#[tokio::test]
async fn issue_hash_sha256_roundtrip() {
    let Some(pool) = pool().await else { return };
    let tenant = probe_tenant();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'probe') ON CONFLICT (id) DO NOTHING")
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
    let store = PgTenantKeyStore::new(pool);
    let raw = store.issue(&tenant).await.unwrap();
    let got = store.tenant_of(&sha256_hex(raw.as_bytes())).await.unwrap();
    assert_eq!(got.as_deref(), Some(tenant.as_str()), "库存哈希必须恰为 sha256_hex(明文)");
}

/// 冻结测试：坏 URL 池 → of/tenant_of 前缀 pg_store_unavailable；纯函数判定成立。
#[tokio::test]
async fn pg_store_unavailable_maps_503() {
    let lazy = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(300))
        .connect_lazy("postgres://postgres:forge@127.0.0.1:1/forge") // 不可达端口
        .expect("lazy pool 构造本身不连接");
    let ks = PgTenantKeyStore::new(lazy.clone());
    let err_key = ks.tenant_of("deadbeef").await.unwrap_err();
    assert!(is_pg_store_unavailable(&err_key), "tenant_of 必须判存储不可用: {err_key}");

    let qs = PgQuotaStore::new(lazy);
    let err_quota = qs.of("default").await.unwrap_err();
    assert!(is_pg_store_unavailable(&err_quota), "of 必须判存储不可用: {err_quota}");
    assert!(
        err_quota.to_string().starts_with("invalid state: pg_store_unavailable"),
        "前缀冻结: {err_quota}"
    );
}

/// 冻结测试：未登记租户 issue/set → InvalidState（FK 拒绝，不静默）。
#[tokio::test]
async fn pg_unknown_tenant_rejected() {
    let Some(pool) = pool().await else { return };
    let ghost = format!("t_ghost_{}", std::process::id());
    let ks = PgTenantKeyStore::new(pool.clone());
    let err = ks.issue(&ghost).await.unwrap_err();
    assert!(matches!(err, ForgeError::InvalidState(ref m) if m.contains("tenant not registered")), "实际: {err}");

    let qs = PgQuotaStore::new(pool);
    let err = qs.set(&ghost, QuotaView { max_concurrent: 1, daily_tasks: 1 }).await.unwrap_err();
    assert!(matches!(err, ForgeError::InvalidState(ref m) if m.contains("tenant not registered")), "实际: {err}");
}
