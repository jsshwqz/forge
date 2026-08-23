//! forge-skill：技能加载器 + 信任校验（PH2-005）。
//!
//! - [`load_skill_into`]：第一阶段行为（不校验签名）
//! - [`trust::load_skill_into_verified`]：按 [`trust::SkillTrustPolicy`]
//!   校验 SHA-256 白名单或 HMAC 分离签名后装载

pub mod loader;
pub mod trust;

pub use loader::{load_skill_into, load_skill_manifest, SkillManifest};
pub use trust::load_skill_into_verified;
pub use trust::{
    checksum_of, hmac_sign, verify_skill, SkillTrustPolicy,
};
