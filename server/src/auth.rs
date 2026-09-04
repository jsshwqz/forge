//! Bearer Token 鉴权中间件（API-002；V5.0 TEN-002 多租户升级）。
//!
//! 环境变量 `FORGE_API_KEY`：
//! - 未设置 → 本地模式，全部放行，注入 [`AuthOutcome::Local`]
//! - 已设置 → 除 `/health` 外全部路由要求 `Authorization: Bearer <key>`；
//!   解析顺序（build_v50.md TEN-002 契约）：Bearer → sha256 查 tenant_keys →
//!   命中返回 `Tenant(id)`；未命中且等于 legacy 单钥 → `Tenant(DEFAULT_TENANT)`；
//!   都未命中 → 401。
//!
//! 密钥比较使用常量时间逐字节异或累积，防时序侧信道。
//! 401 统一文案，不泄露失败原因。
//!
//! 说明：鉴权启用条件沿用 SEC-001 配置面（`FORGE_API_KEY` 已设置）。
//! tenant_keys 仅存哈希（0010_tenant_keys 表），明文钥只在 `issue` 返回值出现一次。

use forge_core::{ForgeError, ForgeResult};
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::AppState;

/// 默认租户（与迁移 0009 种子数据一致）。
pub const DEFAULT_TENANT: &str = "default";

/// 鉴权配置。
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    /// `Some(key)` = 启用鉴权；`None` = 本地模式放行。
    pub api_key: Option<String>,
}

impl AuthConfig {
    /// 从环境变量 `FORGE_API_KEY` 读取。
    pub fn from_env() -> Self {
        Self { api_key: std::env::var("FORGE_API_KEY").ok().filter(|s| !s.is_empty()) }
    }

    /// 是否启用鉴权。
    pub fn is_enabled(&self) -> bool {
        self.api_key.is_some()
    }
}

/// 常量时间比较：逐字节异或累积差异。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// sha256 十六进制摘要（钥哈希口径，与 0010_tenant_keys 存储一致）。
pub(crate) fn sha256_hex(input: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(input);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 租户密钥存储 trait（build_v50.md TEN-002 契约）。
#[async_trait::async_trait]
pub trait TenantKeyStore: Send + Sync {
    async fn tenant_of(&self, key_hash: &str) -> ForgeResult<Option<String>>;
    /// 返回明文钥一次，库存哈希（R1：明文不落日志/错误体）。
    async fn issue(&self, tenant_id: &str) -> ForgeResult<String>;
}

/// 鉴权结果（build_v50.md TEN-002 契约）。
#[derive(Clone, Debug)]
pub enum AuthOutcome {
    /// 已识别租户（含 legacy 单钥映射的 DEFAULT_TENANT）。
    Tenant(String),
    /// 本地模式（未启用鉴权）。
    Local,
}

/// 多租户鉴权解析（build_v50.md TEN-002 契约原样实现）。
///
/// - `bearer` 命中 tenant_keys 哈希 → `Tenant(id)`
/// - 未命中且等于 `legacy_key` → `Tenant(DEFAULT_TENANT)`
/// - 无 Bearer 且未配 legacy 钥 → `Local`（本地模式）
/// - 已配钥但 Bearer 缺失或未知 → `PermissionDenied`（中间件映射 401）
pub async fn authenticate(
    store: &dyn TenantKeyStore,
    bearer: Option<&str>,
    legacy_key: Option<&str>,
) -> ForgeResult<AuthOutcome> {
    match bearer.map(str::trim).filter(|s| !s.is_empty()) {
        Some(key) => {
            let hash = sha256_hex(key.as_bytes());
            if let Some(tenant) = store.tenant_of(&hash).await? {
                return Ok(AuthOutcome::Tenant(tenant));
            }
            if let Some(legacy) = legacy_key {
                let legacy = legacy.trim();
                if !legacy.is_empty() && constant_time_eq(key.as_bytes(), legacy.as_bytes()) {
                    return Ok(AuthOutcome::Tenant(DEFAULT_TENANT.to_string()));
                }
            }
            Err(ForgeError::PermissionDenied("unknown key".into()))
        }
        None => {
            if legacy_key.is_none() {
                Ok(AuthOutcome::Local)
            } else {
                Err(ForgeError::PermissionDenied(
                    "authentication required".into(),
                ))
            }
        }
    }
}

/// 内存租户钥存储（开发/测试用；生产走 0010_tenant_keys 的 PG 实现）。
#[derive(Default)]
pub struct InMemoryTenantKeyStore {
    /// key_hash(sha256 hex) → tenant_id
    keys: std::sync::RwLock<HashMap<String, String>>,
}

static ISSUE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[async_trait::async_trait]
impl TenantKeyStore for InMemoryTenantKeyStore {
    async fn tenant_of(&self, key_hash: &str) -> ForgeResult<Option<String>> {
        Ok(self.keys.read().unwrap().get(key_hash).cloned())
    }

    async fn issue(&self, tenant_id: &str) -> ForgeResult<String> {
        // R1：明文钥只在返回值出现一次，库存哈希
        let n = ISSUE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let raw = {
            let full = sha256_hex(format!("{tenant_id}:{n}:{nanos}").as_bytes());
            full[..32].to_string()
        };
        let hash = sha256_hex(raw.as_bytes());
        self.keys
            .write()
            .unwrap()
            .insert(hash, tenant_id.to_string());
        Ok(raw)
    }
}

/// axum 中间件：鉴权检查（跳过 /health）。
///
/// 解析结果写入 request extensions（[`AuthOutcome`]）供下游租户过滤/配额取用。
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // /health 永远放行
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    let config = state.auth.clone();
    if !config.is_enabled() {
        req.extensions_mut().insert(AuthOutcome::Local);
        return next.run(req).await;
    }

    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string);

    match authenticate(state.tenant_keys.as_ref(), bearer.as_deref(), config.api_key.as_deref())
        .await
    {
        Ok(outcome) => {
            req.extensions_mut().insert(outcome);
            next.run(req).await
        }
        Err(_) => {
            let body = serde_json::json!({
                "error": { "code": "unauthorized", "message": "authentication required" }
            });
            (StatusCode::UNAUTHORIZED, Json(body)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冻结测试（V5-FIX-2a）：legacy 单钥映射 DEFAULT_TENANT。
    #[tokio::test]
    async fn legacy_key_maps_default() {
        let store = InMemoryTenantKeyStore::default();
        let outcome = authenticate(&store, Some("legacy-key"), Some("legacy-key"))
            .await
            .unwrap();
        assert!(matches!(outcome, AuthOutcome::Tenant(t) if t == DEFAULT_TENANT));
    }

    /// 冻结测试（V5-FIX-2a）：tenant_keys 命中返回对应租户。
    #[tokio::test]
    async fn tenant_key_resolves() {
        let store = InMemoryTenantKeyStore::default();
        let raw = store.issue("acme").await.unwrap();
        let outcome = authenticate(&store, Some(&raw), Some("legacy-key"))
            .await
            .unwrap();
        assert!(matches!(outcome, AuthOutcome::Tenant(t) if t == "acme"));
    }

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_diff_length() {
        assert!(!constant_time_eq(b"short", b"a-longer-string"));
    }

    #[test]
    fn constant_time_eq_diff_content() {
        assert!(!constant_time_eq(b"abc", b"abd"));
    }

    #[test]
    fn auth_config_from_env_missing_is_local_mode() {
        // 无 env 时为 None
        std::env::remove_var("__TEST_FORGE_KEY__");
        let cfg = AuthConfig { api_key: std::env::var("__TEST_FORGE_KEY__").ok() };
        assert!(!cfg.is_enabled());
    }

    #[test]
    fn auth_config_enabled_when_key_present() {
        let cfg = AuthConfig { api_key: Some("secret123".into()) };
        assert!(cfg.is_enabled());
    }
}
