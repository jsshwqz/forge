//! V5.0 TEN-001: 租户隔离测试

use forge_core::{TenantId, DEFAULT_TENANT_ID};

#[test]
fn tenant_id_has_ten_prefix() {
    let tid = TenantId::new();
    let s = tid.to_string();
    assert!(s.starts_with("ten_"), "TenantId must start with ten_, got: {}", s);
}

#[test]
fn default_tenant_is_default() {
    assert_eq!(DEFAULT_TENANT_ID, "default");
}

#[test]
fn tenant_id_default_impl() {
    let default_tid = TenantId::default();
    assert!(default_tid.to_string().starts_with("ten_"));
}

#[tokio::test]
async fn tenant_isolation_type_check() {
    let tenant_a = TenantId::new();
    let tenant_b = TenantId::new();
    
    // 不同租户必须有不同 ID
    assert_ne!(tenant_a, tenant_b, "Different TenantIds should not be equal");
}
