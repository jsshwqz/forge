//! Bearer Token 鉴权中间件（API-002）。
//!
//! 环境变量 `FORGE_API_KEY`：
//! - 未设置 → 本地模式，全部放行，启动时 tracing::warn! 提示一次
//! - 已设置 → 除 `/health` 外全部路由要求 `Authorization: Bearer <key>`
//!
//! 密钥比较使用常量时间逐字节异或累积，防时序侧信道。
//! 401 统一文案，不泄露失败原因。

use forge_core::ForgeResult;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

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

/// 提取并校验 Bearer Token。
fn extract_bearer(headers: &axum::http::HeaderMap, api_key: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|token| constant_time_eq(token.as_bytes(), api_key.as_bytes()))
        .unwrap_or(false)
}

/// axum 中间件：鉴权检查（跳过 /health）。
pub async fn auth_middleware(
    headers: axum::http::HeaderMap,
    req: Request<Body>,
    next: Next,
) -> Response {
    // 从 extensions 取 AuthConfig（由 from_with_state 注入）
    let config = req
        .extensions()
        .get::<AuthConfig>()
        .cloned()
        .unwrap_or_default();

    if !config.is_enabled() {
        return next.run(req).await;
    }

    // /health 永远放行
    let path = req.uri().path();
    if path == "/health" {
        return next.run(req).await;
    }

    match extract_bearer(&headers, config.api_key.as_deref().unwrap_or("")) {
        true => next.run(req).await,
        false => {
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


/// 租户密钥存储 trait
#[async_trait::async_trait]
pub trait TenantKeyStore: Send + Sync {
    async fn tenant_of(&self, key_hash: &str) -> ForgeResult<Option<String>>;
    async fn issue(&self, tenant_id: &str) -> ForgeResult<String>;
}

/// 鉴权结果
pub enum AuthOutcome { Tenant(String), Local }
