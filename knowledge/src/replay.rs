//! Session 回放导出（KNW-001）：JSON 归档，离线可复盘。

use forge_core::ForgeResult;
use forge_session::{Session, SessionStore};

/// 回放归档。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ReplayArchive {
    pub session: Session,
    /// 导出时间（RFC3339）。
    pub exported_at: String,
    /// 归档格式版本（向前兼容字段）。
    pub format_version: u32,
}

/// 从 store 拉取会话并生成归档。
pub async fn export_replay(
    store: &dyn SessionStore,
    session_id: &forge_core::SessionId,
) -> ForgeResult<ReplayArchive> {
    let session = store.get(session_id).await?;
    Ok(ReplayArchive {
        session,
        exported_at: chrono::Utc::now().to_rfc3339(),
        format_version: 1,
    })
}

/// 归档序列化为 JSON 字符串（pretty，便于人读 + diff）。
pub fn archive_to_json(archive: &ReplayArchive) -> ForgeResult<String> {
    serde_json::to_string_pretty(archive)
        .map_err(|e| forge_core::ForgeError::InvalidState(format!("replay json: {e}")))
}
