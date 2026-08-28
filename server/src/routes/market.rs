//! V5.0 MKT-001/002: 能力市场目录 + 安装即注册
//!
//! - GET /market/capabilities - 公开只读目录（免鉴权）
//! - GET /market/templates - 已发布模板目录（免鉴权）
//! - POST /market/install - 安装能力（需鉴权）

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use forge_cap::{Capability, CapabilityKind, CapabilityRegistry as _};
use forge_product_instance::TemplateRegistry as _;
use crate::AppState;

/// 市场目录查询参数
#[derive(Deserialize)]
pub struct CapabilitiesQuery {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 { 1 }
fn default_per_page() -> u32 { 20 }

/// 模板目录查询参数
#[derive(Deserialize)]
pub struct TemplatesQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

/// 安装请求体
#[derive(Deserialize)]
pub struct InstallRequest {
    pub name: String,
    pub version: String,
}

/// 市场项（脱敏，不含 entry 细节）
#[derive(serde::Serialize)]
pub struct MarketItem {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: String,
    pub permission: String,
}

impl From<&Capability> for MarketItem {
    fn from(cap: &Capability) -> Self {
        Self {
            name: cap.name.clone(),
            version: cap.version.clone(),
            kind: format!("{:?}", cap.kind),
            description: format!("{} v{}", cap.name, cap.version),
            permission: format!("{:?}", cap.permission),
        }
    }
}

/// GET /market/capabilities
pub async fn list_capabilities(
    State(state): State<AppState>,
    Query(q): Query<CapabilitiesQuery>,
) -> Json<serde_json::Value> {
    let per_page = q.per_page.min(100);
    let page = q.page.max(1);
    let offset = (page - 1) * per_page;

    let all: Vec<Capability> = match q.kind.as_deref() {
        Some("Skill") => state.capabilities.list_by_kind(CapabilityKind::Skill).await.unwrap_or_default(),
        Some("Tool") => state.capabilities.list_by_kind(CapabilityKind::Tool).await.unwrap_or_default(),
        Some("McpServer") => state.capabilities.list_by_kind(CapabilityKind::McpServer).await.unwrap_or_default(),
        Some("Api") => state.capabilities.list_by_kind(CapabilityKind::Api).await.unwrap_or_default(),
        _ => {
            let mut caps = vec![];
            for kind in [CapabilityKind::Skill, CapabilityKind::Tool, CapabilityKind::McpServer, CapabilityKind::Api] {
                caps.extend(state.capabilities.list_by_kind(kind).await.unwrap_or_default());
            }
            caps
        }
    };

    let total = all.len();
    let paginated: Vec<MarketItem> = all.into_iter().map(|c| (&c).into()).skip(offset as usize).take(per_page as usize).collect();

    Json(serde_json::json!({ "items": paginated, "total": total }))
}

/// GET /market/templates
pub async fn list_market_templates(
    State(state): State<AppState>,
    Query(q): Query<TemplatesQuery>,
) -> Json<serde_json::Value> {
    let per_page = q.per_page.min(100);
    let page = q.page.max(1);
    let offset = (page - 1) * per_page;

    let all = state.templates.list().await.unwrap_or_default();
    let published: Vec<_> = all.iter().filter(|t| t.review_verdict == "Pass").collect();
    let total = published.len();
    let paginated = published.into_iter().skip(offset as usize).take(per_page as usize);

    Json(serde_json::json!({
        "items": paginated.map(|t| serde_json::json!({
            "id": t.template.id,
            "name": t.template.name,
            "version": &t.version,
            "description": t.template.manifest_skeleton.description,
        })).collect::<Vec<_>>(),
        "total": total
    }))
}

/// POST /market/install
pub async fn install_capability(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let source = state.capabilities.find_by_name(&req.name).await.map_err(|_| (StatusCode::NOT_FOUND, "capability not found".into()))?;
    let cap = source.into_iter().find(|c| c.version == req.version).ok_or((StatusCode::NOT_FOUND, "version not found".into()))?;

    let existing_caps = state.capabilities.find_by_name(&req.name).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(already) = existing_caps.into_iter().find(|c| c.version == req.version) {
        return Ok(Json(serde_json::json!({ "id": already.id, "installed": true })));
    }

    let mut new_cap = cap;
    new_cap.status = forge_cap::CapabilityStatus::Active;
    let id = state.capabilities.register(new_cap).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "id": id, "installed": true })))
}
