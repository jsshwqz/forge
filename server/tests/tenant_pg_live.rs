//! P2 (TEN-004 续): PG 租户态在缺省运行时中实际接线验证。
//!
//! 验证 new_with_pg() 构造的 AppState 真正使用 PgTenantKeyStore / PgQuotaStore,
//! 而非 InMemory 版本（DS P2 发现："缺省单机租户态仍内存版"）。
//!
//! 测试剧本：
//! 1. 用 new_with_pg() 构造 AppState（PG pool）
//! 2. 注册租户 + issue key → tenant_of 查回 → 跨实例可见
//! 3. quota of(默认值) → set → of(新值) → 跨实例可见
//! 4. 重启模拟：第二个 AppState 实例（新 pool 连接）仍能读到 tenant_key 和 quota
//!
//! PG 不可用时自动跳过（同 pg_persistence.rs 约定）。

use forge_server::AppState;
use forge_storage::{connect_and_migrate, PgSessionStore, PgTaskStore};
use std::sync::Arc;

/// 两个独立 PG 连接池模拟进程重启，验证 tenant_keys / quotas 真持久化。
#[tokio::test]
async fn tenant_keys_and_quotas_survive_restart_via_new_with_pg() {
    let Ok(url) = std::env::var("FORGE_PG_URL") else {
        eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
        return;
    };

    // —— 实例 A：注册租户 + issue key + set quota ——
    let pool_a = connect_and_migrate(&url).await.unwrap();
    let state_a = AppState::new_with_pg(
        Arc::new(PgTaskStore::new(pool_a.clone())),
        Arc::new(PgSessionStore::new(pool_a.clone())),
        pool_a.clone(),
    );

    // 注册租户 (插入 tenants 表)
    let tenant_id = format!("tenant-p2-{}", std::process::id());
    sqlx::query("INSERT INTO tenants (tenant_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(&tenant_id)
        .bind("P2 test tenant")
        .execute(&pool_a)
        .await
        .unwrap();

    // issue key — 用 AppState 的 tenant_keys (应该是 PgTenantKeyStore)
    let raw_key = state_a.tenant_keys.issue(&tenant_id).await.unwrap();
    assert!(!raw_key.is_empty(), "issue key must not be empty");

    // tenant_of — 验证 PG 查回
    let hash = forge_server::auth::sha256_hex(raw_key.as_bytes());
    let found = state_a.tenant_keys.tenant_of(&hash).await.unwrap();
    assert_eq!(
        found.as_deref(),
        Some(tenant_id.as_str()),
        "tenant_of must return the tenant_id from PG"
    );

    // quota of(默认值) — 新租户应返回 DEFAULT_QUOTA
    let q_default = state_a.quotas.of(&tenant_id).await.unwrap();
    assert_eq!(q_default.max_concurrent, 4, "default max_concurrent");
    assert_eq!(q_default.daily_tasks, 100, "default daily_tasks");

    // set quota — 新值
    let new_quota = forge_server::quota::QuotaView {
        max_concurrent: 8,
        daily_tasks: 200,
    };
    state_a.quotas.set(&tenant_id, new_quota.clone()).await.unwrap();

    // 验证 set 生效
    let q_after = state_a.quotas.of(&tenant_id).await.unwrap();
    assert_eq!(q_after.max_concurrent, 8, "set max_concurrent");
    assert_eq!(q_after.daily_tasks, 200, "set daily_tasks");

    // —— 实例 B：新连接池模拟重启 ——
    let pool_b = connect_and_migrate(&url).await.unwrap();
    let state_b = AppState::new_with_pg(
        Arc::new(PgTaskStore::new(pool_b.clone())),
        Arc::new(PgSessionStore::new(pool_b.clone())),
        pool_b,
    );

    // tenant_of — 第二个实例仍能查到 key (PG 持久化)
    let found_b = state_b.tenant_keys.tenant_of(&hash).await.unwrap();
    assert_eq!(
        found_b.as_deref(),
        Some(tenant_id.as_str()),
        "tenant_key must survive restart (PG-backed)"
    );

    // quota of — 第二个实例仍能读到 set 的新值 (PG 持久化)
    let q_b = state_b.quotas.of(&tenant_id).await.unwrap();
    assert_eq!(q_b.max_concurrent, 8, "quota must survive restart");
    assert_eq!(q_b.daily_tasks, 200, "quota must survive restart");

    // 清理
    sqlx::query("DELETE FROM tenant_keys WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(state_a.pool.as_ref().unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM quotas WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(state_a.pool.as_ref().unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM tenants WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(state_a.pool.as_ref().unwrap())
        .await
        .unwrap();

    println!("[P2] ✅ tenant_keys + quotas 真正用 PG 实现, 跨实例重启持久化验证通过");
}

/// 验证 in_memory() 构造器仍用 InMemory（29 个测试调用点依赖此行为）。
#[tokio::test]
async fn in_memory_state_uses_in_memory_tenant_and_quota() {
    let state = AppState::in_memory();

    // issue + tenant_of 应工作（InMemory 版本）
    let tenant_id = "test-in-memory-tenant";
    let raw_key = state.tenant_keys.issue(tenant_id).await.unwrap();
    assert!(!raw_key.is_empty());

    let hash = forge_server::auth::sha256_hex(raw_key.as_bytes());
    let found = state.tenant_keys.tenant_of(&hash).await.unwrap();
    assert_eq!(found.as_deref(), Some(tenant_id));

    // quota 默认值
    let q = state.quotas.of(tenant_id).await.unwrap();
    assert_eq!(q.max_concurrent, 4);
    assert_eq!(q.daily_tasks, 100);

    // pool 应为 None（InMemory 模式）
    assert!(state.pool.is_none(), "in_memory() must have pool=None");

    println!("[P2] ✅ in_memory() 仍用 InMemory tenant/quota (测试依赖保持不变)");
}
