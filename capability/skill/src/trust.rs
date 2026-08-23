//! Skill 信任策略（PH2-005，B-06 签名/完整性校验）。
//!
//! 三种模式：
//! - [`SkillTrustPolicy::Disabled`]           不校验（第一阶段默认行为）
//! - [`SkillTrustPolicy::ChecksumWhitelist`]  skill.json 的 SHA-256 必须在白名单内
//! - [`SkillTrustPolicy::HmacKey`]            存在分离签名文件 `skill.sig`
//!   （内容 = hex(HMAC-SHA256(key, skill.json 原始字节))），不匹配即拒绝
//!
//! 选型理由（R6，2026-08-23）：当前无 PKI/KMS 基础设施，HMAC 共享密钥 +
//! 白名单是最小可信方案；非对称签名（ed25519）待引入密钥分发体系后升级。
//!
//! 失败语义：任何不匹配 → `ForgeError::PermissionDenied`（明确拒绝，不降级放行）。
//! 密钥来源：运行时环境变量注入（如 FORGE_SKILL_HMAC_KEY），禁止硬编码入库。

use crate::loader::SkillManifest;
use forge_core::{ForgeError, ForgeResult};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::Path;

/// 信任策略。
#[derive(Clone, Debug)]
pub enum SkillTrustPolicy {
    /// 跳过校验。
    Disabled,
    /// 允许的 skill.json SHA-256（小写十六进制）列表。
    ChecksumWhitelist(Vec<String>),
    /// HMAC-SHA256 共享密钥；要求目录下存在 `skill.sig` 分离签名。
    HmacKey(String),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 计算 skill.json 的 SHA-256（小写十六进制）。
pub fn checksum_of(raw: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(raw);
    hex(&h.finalize())
}

/// 计算 HMAC-SHA256 签名（小写十六进制）——供签发方与校验方共用。
pub fn hmac_sign(key: &str, raw: &[u8]) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .expect("hmac accepts any key length");
    m.update(raw);
    hex(&m.finalize().into_bytes())
}

/// 按策略校验目录中的 skill 完整性。通过返回解析出的 manifest 便于复用。
pub fn verify_skill(dir: &Path, policy: &SkillTrustPolicy) -> ForgeResult<SkillManifest> {
    let path = dir.join("skill.json");
    match policy {
        SkillTrustPolicy::Disabled => crate::loader::load_skill_manifest(dir),

        SkillTrustPolicy::ChecksumWhitelist(list) => {
            let raw = std::fs::read(&path)
                .map_err(|e| ForgeError::NotFound(format!("read {}: {e}", path.display())))?;
            let sum = checksum_of(&raw);
            if list.iter().any(|w| w.eq_ignore_ascii_case(&sum)) {
                crate::loader::load_skill_manifest(dir)
            } else {
                Err(ForgeError::PermissionDenied(format!(
                    "skill checksum {sum} not in whitelist ({})",
                    path.display()
                )))
            }
        }

        SkillTrustPolicy::HmacKey(key) => {
            let raw = std::fs::read(&path)
                .map_err(|e| ForgeError::NotFound(format!("read {}: {e}", path.display())))?;
            let sig_path = dir.join("skill.sig");
            let provided = std::fs::read_to_string(&sig_path).map_err(|e| {
                ForgeError::NotFound(format!("read {}: {e}", sig_path.display()))
            })?;
            let expected = hmac_sign(key, &raw);
            // 去除尾随空白/换行后比较（签发工具常带换行）
            if provided.trim().eq_ignore_ascii_case(&expected) {
                crate::loader::load_skill_manifest(dir)
            } else {
                Err(ForgeError::PermissionDenied(format!(
                    "skill signature mismatch ({})",
                    sig_path.display()
                )))
            }
        }
    }
}

/// 带信任策略的装载入口：verify 通过后注册为能力。
///
/// 原 [`crate::load_skill_into`] 保持第一阶段行为等价于 `Disabled`。
pub async fn load_skill_into_verified(
    dir: &Path,
    registry: &dyn forge_cap::CapabilityRegistry,
    policy: &SkillTrustPolicy,
) -> ForgeResult<forge_core::CapabilityId> {
    let manifest = verify_skill(dir, policy)?;
    let cap = forge_cap::Capability {
        id: forge_core::CapabilityId::new_capability_id(),
        name: manifest.name,
        kind: forge_cap::CapabilityKind::Skill,
        version: manifest.version,
        entry: manifest.entry,
        status: forge_cap::CapabilityStatus::Registered,
        permission: manifest.permission,
    };
    registry.register(cap).await
}
