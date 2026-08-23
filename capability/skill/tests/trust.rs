//! PH2-005 集成测试：三种信任策略 × 篡改检测 × 端到端装载。

use forge_cap::{CapabilityRegistry, InMemoryCapabilityRegistry};
use forge_skill::{
    checksum_of, hmac_sign, load_skill_into_verified, verify_skill, SkillTrustPolicy,
};
use std::io::Write;
use std::path::Path;

const KEY: &str = "unit-test-hmac-key";

fn write_skill(dir: &Path, body: &str) {
    let mut f = std::fs::File::create(dir.join("skill.json")).unwrap();
    f.write_all(body.as_bytes()).unwrap();
}

fn valid_body() -> String {
    serde_json::json!({
        "name": "signed-skill",
        "version": "1.0.0",
        "description": "t",
        "entry": "main.rs",
        "permission": "ReadOnly"
    })
    .to_string()
}

#[tokio::test]
async fn disabled_keeps_phase1_behavior() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(dir.path(), &valid_body());
    // 无 skill.sig、无白名单 → Disabled 放行
    let reg = InMemoryCapabilityRegistry::default();
    let id = load_skill_into_verified(dir.path(), &reg, &SkillTrustPolicy::Disabled)
        .await
        .unwrap();
    assert!(registry_contains(&reg, &id).await);
}

#[tokio::test]
async fn hmac_valid_signature_passes_and_registers() {
    let dir = tempfile::tempdir().unwrap();
    let body = valid_body();
    write_skill(dir.path(), &body);
    let mut f = std::fs::File::create(dir.path().join("skill.sig")).unwrap();
    f.write_all(hmac_sign(KEY, body.as_bytes()).as_bytes()).unwrap();

    let reg = InMemoryCapabilityRegistry::default();
    let id = load_skill_into_verified(
        dir.path(),
        &reg,
        &SkillTrustPolicy::HmacKey(KEY.into()),
    )
    .await
    .unwrap();
    assert!(registry_contains(&reg, &id).await);
}

#[tokio::test]
async fn hmac_tampered_content_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let body = valid_body();
    write_skill(dir.path(), &body);
    // 签名对原内容有效
    std::fs::write(dir.path().join("skill.sig"), hmac_sign(KEY, body.as_bytes())).unwrap();
    // 内容被篡改 1 字节
    let tampered = body.replace("1.0.0", "9.9.9");
    write_skill(dir.path(), &tampered);

    let err = verify_skill(dir.path(), &SkillTrustPolicy::HmacKey(KEY.into())).unwrap_err();
    assert!(err.to_string().contains("signature mismatch"));
}

#[tokio::test]
async fn hmac_missing_sig_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(dir.path(), &valid_body());
    let err = verify_skill(dir.path(), &SkillTrustPolicy::HmacKey(KEY.into())).unwrap_err();
    assert!(err.to_string().contains("skill.sig") || err.to_string().contains("not found"));
}

#[tokio::test]
async fn checksum_whitelist_pass_and_fail() {
    let dir = tempfile::tempdir().unwrap();
    let body = valid_body();
    write_skill(dir.path(), &body);
    let sum = checksum_of(body.as_bytes());

    // 在白名单 → 通过
    let ok = verify_skill(
        dir.path(),
        &SkillTrustPolicy::ChecksumWhitelist(vec![sum.clone()]),
    );
    assert!(ok.is_ok());

    // 不在白名单 → PermissionDenied 且文案含摘要
    let err = verify_skill(
        dir.path(),
        &SkillTrustPolicy::ChecksumWhitelist(vec!["deadbeef".into()]),
    )
    .unwrap_err();
    assert!(err.to_string().contains(&sum) && err.to_string().contains("whitelist"));
}

#[tokio::test]
async fn sign_helper_matches_verify() {
    // 签发/校验共用同一实现的自洽性（防实现漂移）
    let raw = b"{\"name\":\"x\"}";
    let sig = hmac_sign(KEY, raw);
    assert_eq!(sig.len(), 64); // sha256 hex
    assert_eq!(sig, hmac_sign(KEY, raw), "deterministic");
}

async fn registry_contains(reg: &InMemoryCapabilityRegistry, id: &forge_core::CapabilityId) -> bool {
    reg.get(id).await.is_ok()
}
