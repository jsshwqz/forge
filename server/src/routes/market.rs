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
///
/// 语义（V5-FIX-2c）：同库检索 → 命中且 Active → 幂等返回；
/// 命中且非 Active → 置 Active 后返回；未命中 → 404。
pub async fn install_capability(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let caps = state.capabilities.find_by_name(&req.name).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // MKT-102：version 字段语义升级——精确版钉版安装；约束版（^0.1/~0.1/*）解析
    let yanked: std::collections::HashSet<String> = if let Some(pool) = state.pool.clone() {
        sqlx::query_as("SELECT version FROM releases WHERE name = $1 AND yanked = true")
            .bind(&req.name)
            .fetch_all(&pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into_iter()
            .map(|(v,): (String,)| v)
            .collect()
    } else {
        Default::default()
    };

    let target: String = if let Ok(pinned) = semver::Version::parse(&req.version) {
        // R2 钉版命中 yanked → 409（显式优于静默）
        if yanked.contains(&req.version) {
            return Err((StatusCode::CONFLICT, "version yanked".into()));
        }
        pinned.to_string()
    } else if let Ok(_req) = semver::VersionReq::parse(&req.version) {
        // R1：resolve 排除 yanked/deprecated（flags 来自 releases 表）
        let mut metas: Vec<forge_cap::versioning::VersionMeta> = Vec::new();
        if let Some(pool) = state.pool.clone() {
            let flags: Vec<(String, bool, bool)> = sqlx::query_as(
                "SELECT version, yanked, deprecated FROM releases WHERE name = $1",
            )
            .bind(&req.name)
            .fetch_all(&pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            for c in &caps {
                if let Ok(v) = semver::Version::parse(&c.version) {
                    let (y, d) = flags
                        .iter()
                        .find(|(ver, _, _)| *ver == c.version)
                        .map(|(_, y, d)| (*y, *d))
                        .unwrap_or((false, false));
                    metas.push(forge_cap::versioning::VersionMeta { version: v, yanked: y, deprecated: d });
                }
            }
        } else {
            for c in &caps {
                if let Ok(v) = semver::Version::parse(&c.version) {
                    metas.push(forge_cap::versioning::VersionMeta::clean(v));
                }
            }
        }
        match forge_cap::versioning::resolve_version_meta(&req.version, &metas) {
            Some(v) => v.to_string(),
            None => return Err((StatusCode::NOT_FOUND, "no compatible version".into())),
        }
    } else {
        // 非 semver 字符串：保持既有精确匹配行为
        req.version.clone()
    };

    let cap = caps.into_iter().find(|c| c.version == target).ok_or((StatusCode::NOT_FOUND, "capability not found".into()))?;

    // MKT-104B: 制品 hash 复核（先快后慢: sha256 本地计算 → ed25519 验签查找）
    // 存在 artifact_path 的 release 必须盘上字节 sha256 与存储时 artifact_sha256 一致
    if let Some(pool) = state.pool.clone() {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT artifact_path, artifact_sha256 FROM releases \
             WHERE name = $1 AND version = $2 ORDER BY id DESC LIMIT 1",
        )
        .bind(&req.name)
        .bind(&req.version)
        .fetch_optional(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if let Some((Some(artifact_path), Some(artifact_sha256))) = row {
            use sha2::{Digest, Sha256};
            let id = forge_core::ArtifactId::from(artifact_path);
            let bytes = state.artifact_store.read(&id).await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("artifact read: {e}")))?;
            let mut h = Sha256::new();
            h.update(&bytes);
            let actual: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if actual != artifact_sha256 {
                return Err((StatusCode::CONFLICT, "artifact hash mismatch".into()));
            }
        }
        // artifact_path = None (旧兼容) 或无 release 行 → 跳过 hash 复核
    }

    // MKT-101 R2 出站复验：存在 release 记录的 (name, version) 必须验签通过
    if let Some(pool) = state.pool.clone() {
        install_signature_recheck(&pool, &req.name, &req.version).await?;
    }

    if cap.status == forge_cap::CapabilityStatus::Active {
        return Ok(Json(serde_json::json!({ "id": cap.id, "installed": true })));
    }
    state
        .capabilities
        .set_status(&cap.id, forge_cap::CapabilityStatus::Active)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "id": cap.id, "installed": true })))
}

// ==================== MKT-101：发布者生态与签名（V6.0） ====================

use axum::http::HeaderMap;
use forge_cap::signing;

/// 签名覆盖的规范字节：`{name}\n{version}\n{package_hash}`（发布与安装复验一致）。
fn package_bytes(name: &str, version: &str, package_hash: &str) -> Vec<u8> {
    format!("{name}\n{version}\n{package_hash}").into_bytes()
}

#[derive(serde::Deserialize)]
pub struct PublishRequest {
    pub name: String,
    pub version: String,
    pub package_hash: String,
    pub signature: String,
    /// MKT-104A: base64 编码的制品字节（可选；None 时保持旧元数据行为，向后兼容）。
    #[serde(default)]
    pub package_data: Option<String>,
}

/// POST /market/publish — 发布者提交 release（Publisher-Key 头 = publisher_id）。
///
/// R2 入站验签：签名必须匹配该 publisher 登记公钥；未登记 publisher → 403。
/// R3：私钥永不经过服务端（发布者本地签名），这里只接触公钥与签名。
pub async fn publish_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PublishRequest>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let Some(pool) = state.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "releases require PostgreSQL storage".into()));
    };
    let publisher_id = headers
        .get("Publisher-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    if publisher_id.is_empty() {
        return Err((StatusCode::FORBIDDEN, "missing Publisher-Key".into()));
    }
    let pk: Option<(String,)> = sqlx::query_as(
        "SELECT public_key FROM publisher_keys WHERE publisher_id = $1",
    )
    .bind(&publisher_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((public_key,)) = pk else {
        return Err((StatusCode::FORBIDDEN, "unknown publisher".into()));
    };

    // R2 入站验签：伪造/错误签名一律 403
    let bytes = package_bytes(&req.name, &req.version, &req.package_hash);
    let ok = signing::verify_package(&public_key, &bytes, &req.signature)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !ok {
        return Err((StatusCode::FORBIDDEN, "signature verification failed".into()));
    }

    // MKT-104A: 制品字节上传 + 服务端实测 hash 复核（D-5 缺口上游封堵）
    // package_data = Some → 解码 → 限长 → ArtifactStore.put → 实测 sha256 与声称 package_hash 比对
    // package_data = None → 旧元数据行为（三新列 NULL，向后兼容）
    let (artifact_path, artifact_size, artifact_sha256): (Option<String>, Option<i64>, Option<String>) =
        if let Some(data) = &req.package_data {
            use base64::prelude::*;

            let content = BASE64_STANDARD.decode(data)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("base64 decode failed: {e}")))?;

            // 限长：FORGE_PACKAGE_MAX_BYTES（缺省 16MB）
            let max_bytes = std::env::var("FORGE_PACKAGE_MAX_BYTES")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(16_777_216);
            if content.len() > max_bytes {
                return Err((StatusCode::PAYLOAD_TOO_LARGE, "artifact exceeds FORGE_PACKAGE_MAX_BYTES".into()));
            }

            // ArtifactStore.put 内部计算 sha256（trait 契约：调用方不传 hash）
            let artifact = state.artifact_store.put(
                req.name.clone(),
                forge_storage::ArtifactKind::Binary,
                content,
                serde_json::json!({}),
            ).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("artifact store: {e}")))?;

            // D-5 修复：服务端实测 hash 与发布者声称 package_hash 比对
            if artifact.checksum_sha256 != req.package_hash {
                return Err((StatusCode::CONFLICT, "artifact hash mismatch".into()));
            }

            // artifact_path 存储 ArtifactId（通过 trait read 方法检索制品字节）
            (Some(artifact.id.as_ref().to_string()), Some(artifact.size_bytes as i64), Some(artifact.checksum_sha256.clone()))
        } else {
            (None, None, None)
        };

    let (release_id,): (i64,) = sqlx::query_as(
        "INSERT INTO releases (name, version, publisher_id, package_hash, signature, artifact_path, artifact_size, artifact_sha256) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&req.name)
    .bind(&req.version)
    .bind(&publisher_id)
    .bind(&req.package_hash)
    .bind(&req.signature)
    .bind(&artifact_path)
    .bind(artifact_size)
    .bind(&artifact_sha256)
    .fetch_one(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(serde_json::json!({ "release_id": release_id, "review_status": "pending" })),
    ))
}

/// GET /market/releases/{name}/{version}/download — 制品下载（匿名允许, D7）。
///
/// 可见性过滤与 list_releases 同口径：yanked → 409。
/// artifact_path 为 NULL → 410（无制品字节，仅元数据发布的旧 release）。
/// 命中 → 200 application/octet-stream，响应前对读出资节再实测一次 sha256 与 artifact_sha256 比对（盘后读损坏防线）。
pub async fn download_release(
    State(state): State<AppState>,
    axum::extract::Path((name, version)): axum::extract::Path<(String, String)>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    use axum::body::Body;
    use sha2::{Digest, Sha256};

    let Some(pool) = state.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "releases require PostgreSQL storage".into()));
    };

    // 查询 release（与 list_releases 同口径：yanked 不可见 → 409）
    type ReleaseRow = (Option<String>, Option<String>, Option<i64>, bool);
    let row: Option<ReleaseRow> = sqlx::query_as(
        "SELECT artifact_path, artifact_sha256, artifact_size, yanked FROM releases \
         WHERE name = $1 AND version = $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(&name)
    .bind(&version)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((artifact_path, artifact_sha256, _artifact_size, yanked)) = row else {
        return Err((StatusCode::NOT_FOUND, "release not found".into()));
    };

    if yanked {
        return Err((StatusCode::CONFLICT, "version yanked".into()));
    }

    let Some(path) = artifact_path else {
        return Err((StatusCode::GONE, "artifact not found".into()));
    };

    // 通过 ArtifactStore::read 检索制品字节（trait 抽象, D5=方案B FileArtifactStore）
    let id = forge_core::ArtifactId::from(path);
    let bytes = state.artifact_store.read(&id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("artifact read: {e}")))?;

    // 盘后读损坏防线：读出资节再实测 sha256
    if let Some(expected_sha) = &artifact_sha256 {
        let mut h = Sha256::new();
        h.update(&bytes);
        let actual: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if &actual != expected_sha {
            eprintln!("[market] artifact sha256 mismatch on download: {name}/{version} expected={expected_sha} actual={actual}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "artifact integrity check failed".into()));
        }
    }

    // 200 octet-stream
    let content_length = bytes.len();
    let response = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .header("content-length", content_length.to_string())
        .body(Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("response build: {e}")))?;
    Ok(response)
}

#[derive(serde::Deserialize)]
pub struct ReviewRequest {
    pub release_id: i64,
    pub verdict: String,
}

/// POST /market/review — 审核裁决（仅 DEFAULT_TENANT 管理员；Local 模式视为 default）。
///
/// R1 状态机：pending→approved（自动转 published）/ pending→rejected（终态）；
/// 其余迁移一律拒绝（409）。
pub async fn review_release(
    State(state): State<AppState>,
    outcome: axum::Extension<crate::auth::AuthOutcome>,
    Json(req): Json<ReviewRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // 管理员门禁：Tenant 仅 default；Local（本地模式）视为 default 管理员
    let tenant = match outcome.0 {
        crate::auth::AuthOutcome::Tenant(t) => t,
        crate::auth::AuthOutcome::Local => crate::auth::DEFAULT_TENANT.to_string(),
    };
    if tenant != crate::auth::DEFAULT_TENANT {
        return Err((StatusCode::FORBIDDEN, "review requires default tenant admin".into()));
    }
    let Some(pool) = state.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "releases require PostgreSQL storage".into()));
    };

    let (status,): (String,) = sqlx::query_as(
        "SELECT review_status FROM releases WHERE id = $1",
    )
    .bind(req.release_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((StatusCode::NOT_FOUND, "release not found".into()))?;

    // 先校验 verdict 合法性（未知裁决一律 400），再做状态机迁移
    let new_status = match req.verdict.as_str() {
        "approved" => "published", // R1：approved 自动转 published
        "rejected" => "rejected",
        other => return Err((StatusCode::BAD_REQUEST, format!("invalid verdict: {other}"))),
    };
    if status != "pending" {
        return Err((
            StatusCode::CONFLICT,
            format!("invalid transition: {status} is terminal or already reviewed"),
        ));
    }
    sqlx::query("UPDATE releases SET review_status = $2 WHERE id = $1")
        .bind(req.release_id)
        .bind(new_status)
        .execute(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "release_id": req.release_id, "review_status": new_status })))
}

/// GET /market/releases?name= — release 目录（公开只读）。
pub async fn list_releases(
    State(state): State<AppState>,
    Query(q): Query<CapabilitiesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let Some(pool) = state.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "releases require PostgreSQL storage".into()));
    };
    // R1：yanked 对目录列表不可见；deprecated 可见（带标记，resolve 侧排除）
    let rows: Vec<(String, String, String, String, String, bool)> = if let Some(name) = &q.kind {
        // kind 字段在 releases 语境下复用为 name 过滤（保持分页参数一致）
        sqlx::query_as(
            "SELECT name, version, publisher_id, review_status, package_hash, deprecated FROM releases \
             WHERE yanked = false AND name = $1 ORDER BY id",
        )
        .bind(name)
        .fetch_all(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as(
            "SELECT name, version, publisher_id, review_status, package_hash, deprecated FROM releases \
             WHERE yanked = false ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(name, version, publisher_id, review_status, package_hash, deprecated)| {
            serde_json::json!({ "name": name, "version": version, "publisher_id": publisher_id,
                                "review_status": review_status, "package_hash": package_hash,
                                "deprecated": deprecated })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items, "total": items.len() })))
}

/// R2 出站复验（MKT-002 install）：存在 release 记录的 (name, version) 必须验签通过；
/// 无 release 记录的既有内存能力不受影响。
pub(crate) async fn install_signature_recheck(
    pool: &sqlx::PgPool,
    name: &str,
    version: &str,
) -> Result<(), (StatusCode, String)> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT r.signature, r.package_hash, pk.public_key FROM releases r \
         JOIN publisher_keys pk ON pk.publisher_id = r.publisher_id \
         WHERE r.name = $1 AND r.version = $2 ORDER BY r.id DESC LIMIT 1",
    )
    .bind(name)
    .bind(version)
    .fetch_optional(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((signature, package_hash, public_key)) = row else {
        return Ok(()); // 无 release 记录：非签名发布路径
    };
    let bytes = package_bytes(name, version, &package_hash);
    let ok = signing::verify_package(&public_key, &bytes, &signature)
        .map_err(|e| (StatusCode::FORBIDDEN, e.to_string()))?;
    if !ok {
        return Err((StatusCode::FORBIDDEN, "install rejected: invalid release signature".into()));
    }
    Ok(())
}

/// DELETE /market/releases/{name}/{version} — 删除 release（制品字节 + 元数据同删）。
///
/// 鉴权: Publisher-Key 必须匹配 release 的 publisher_id。
/// 制品字节通过 ArtifactStore::delete 清理（幂等）; PG 行通过 DELETE 清理。
pub async fn delete_release(
    State(state): State<AppState>,
    axum::extract::Path((name, version)): axum::extract::Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let Some(pool) = state.pool.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "releases require PostgreSQL storage".into()));
    };

    let publisher_id = headers
        .get("Publisher-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    if publisher_id.is_empty() {
        return Err((StatusCode::FORBIDDEN, "missing Publisher-Key".into()));
    }

    // 查询 release
    let row: Option<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, publisher_id, artifact_path FROM releases \
         WHERE name = $1 AND version = $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(&name)
    .bind(&version)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((release_id, row_publisher_id, artifact_path)) = row else {
        return Err((StatusCode::NOT_FOUND, "release not found".into()));
    };

    // 鉴权: 只有发布者本人可删
    if row_publisher_id != publisher_id {
        return Err((StatusCode::FORBIDDEN, "not the release publisher".into()));
    }

    // 删制品文件（幂等; 失败不阻塞 PG 行删除——孤儿文件由 D9 清理策略兜底）
    if let Some(path) = artifact_path {
        let id = forge_core::ArtifactId::from(path);
        if let Err(e) = state.artifact_store.delete(&id).await {
            eprintln!("[market] artifact delete failed for {name}/{version}: {e}");
        }
    }

    // 删 PG 行
    let _ = sqlx::query("DELETE FROM releases WHERE id = $1")
        .bind(release_id)
        .execute(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
