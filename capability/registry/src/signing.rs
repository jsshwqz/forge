//! V6.0 MKT-101：能力包 ed25519 签名/验签（build_v60.md AF-BP-V60A 契约）。
//!
//! 安全规则：
//! - R3 私钥永不入库、永不入日志、永不出现在任何响应体；
//! - R4 公钥指纹（pk_hex 前 16 字符）可入日志用于审计；
//! - 验签失败返回 `Ok(false)`，仅格式错误（hex 非法/长度错）返回 `Err`。

use ed25519_dalek::{Signer, Verifier};
use forge_core::{ForgeError, ForgeResult};

/// hex 编码（小写）。
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// hex 解码；非法字符/奇数长度 → Err（R2：格式错误面）。
fn from_hex(s: &str) -> ForgeResult<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(ForgeError::InvalidState("hex: odd length".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| ForgeError::InvalidState(format!("hex: invalid byte at {i}")))
        })
        .collect()
}

/// 生成 ed25519 密钥对，返回 (sk_hex, pk_hex)。
///
/// R3：明文私钥只在返回值出现一次，调用方负责安全处置（不得入库/入日志）。
pub fn generate_keypair() -> (String, String) {
    let mut secret = [0u8; 32];
    use rand_core::RngCore;
    rand_core::OsRng.fill_bytes(&mut secret);
    let signing = ed25519_dalek::SigningKey::from_bytes(&secret);
    (to_hex(&signing.to_bytes()), to_hex(&signing.verifying_key().to_bytes()))
}

/// 签名 package 字节，返回 sig_hex（64 字节 ed25519 签名）。
pub fn sign_package(sk_hex: &str, package_bytes: &[u8]) -> ForgeResult<String> {
    let sk = from_hex(sk_hex)?;
    let arr: [u8; 32] = sk
        .try_into()
        .map_err(|_| ForgeError::InvalidState("signing key must be 32 bytes hex".into()))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&arr);
    Ok(to_hex(&signing.sign(package_bytes).to_bytes()))
}

/// 验签：失败（签名不匹配）→ `Ok(false)`；hex 非法/长度错 → `Err`。
pub fn verify_package(pk_hex: &str, package_bytes: &[u8], sig_hex: &str) -> ForgeResult<bool> {
    let pk = from_hex(pk_hex)?;
    let arr: [u8; 32] = pk
        .try_into()
        .map_err(|_| ForgeError::InvalidState("public key must be 32 bytes hex".into()))?;
    let verifying = ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map_err(|e| ForgeError::InvalidState(format!("public key: {e}")))?;
    let sig = from_hex(sig_hex)?;
    let arr64: [u8; 64] = sig
        .try_into()
        .map_err(|_| ForgeError::InvalidState("signature must be 64 bytes hex".into()))?;
    let signature = ed25519_dalek::Signature::from_bytes(&arr64);
    Ok(verifying.verify(package_bytes, &signature).is_ok())
}

/// 公钥指纹（pk_hex 前 16 字符，R4：可入日志）。
pub fn pk_fingerprint(pk_hex: &str) -> String {
    pk_hex.chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冻结测试（MKT-101）：生成 → 签名 → 验签 true。
    #[test]
    fn sign_verify_roundtrip() {
        let (sk, pk) = generate_keypair();
        let package = b"capability package bytes v1";
        let sig = sign_package(&sk, package).unwrap();
        assert_eq!(sig.len(), 128, "64 字节签名的 hex 形态");
        assert!(verify_package(&pk, package, &sig).unwrap(), "roundtrip 必须验签通过");
    }

    /// 冻结测试（MKT-101）：改 1 字节 → verify false。
    #[test]
    fn tampered_package_rejected() {
        let (sk, pk) = generate_keypair();
        let package = b"capability package bytes v1";
        let sig = sign_package(&sk, package).unwrap();
        let mut tampered = package.to_vec();
        tampered[0] ^= 0x01;
        assert!(
            !verify_package(&pk, &tampered, &sig).unwrap(),
            "篡改 1 字节必须验签失败"
        );
    }

    #[test]
    fn format_errors_are_err_not_false() {
        let (_, pk) = generate_keypair();
        // 非法 hex
        assert!(verify_package(&pk, b"x", "zz").is_err());
        // 长度错（签名非 64 字节）
        assert!(verify_package(&pk, b"x", "abcd").is_err());
        // 私钥长度错
        assert!(sign_package("abcd", b"x").is_err());
    }

    #[test]
    fn fingerprint_is_16_chars() {
        let (_, pk) = generate_keypair();
        assert_eq!(pk_fingerprint(&pk).len(), 16);
    }
}
