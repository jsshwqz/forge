//! MinIO(S3 兼容) 产物存储：ArtifactStore 的对象存储实现（PH2-001b）。
//!
//! 手写 AWS Signature V4（path-style，仅 PUT/GET/HEAD + 建桶），
//! 依赖刻意最小化：`hmac` + 既有 `sha2` + 无默认特性的 `reqwest`（纯 HTTP）。
//!
//! 元数据映射：
//! - key           = ArtifactId 字符串
//! - x-amz-meta-name / -kind / -checksum / -created → Artifact 字段
//!
//! 区域固定 us-east-1（MinIO 默认接受任意值）。

use async_trait::async_trait;
use chrono::Utc;
use forge_artifact::{Artifact, ArtifactKind, ArtifactStore};
use forge_core::{ArtifactId, ForgeError, ForgeResult};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const REGION: &str = "us-east-1";
const SERVICE: &str = "s3";

/// MinIO/S3 配置。
#[derive(Clone, Debug)]
pub struct S3Config {
    /// 形如 `http://localhost:19000`。
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
}

impl S3Config {
    /// 校验必填字段。
    pub fn validate(&self) -> ForgeResult<()> {
        if self.endpoint.is_empty() || self.bucket.is_empty()
            || self.access_key.is_empty() || self.secret_key.is_empty() {
            return Err(ForgeError::InvalidState(
                "S3Config: endpoint/bucket/access_key/secret_key must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// MinIO 产物存储。
pub struct MinioArtifactStore {
    http: reqwest::Client,
    cfg: S3Config,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// 计算 SigV4 Authorization 头（内部函数，单测覆盖确定性）。
#[allow(clippy::too_many_arguments)]
fn sigv4_auth_header(
    method: &str,
    canonical_uri: &str,
    host: &str,
    payload_hash: &str,
    amz_date: &str,
    date_stamp: &str,
    access_key: &str,
    secret_key: &str,
) -> String {
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{method}\n{uri}\n\nhost:{host}\nx-amz-content-sha256:{ph}\nx-amz-date:{ad}\n\n{sh}\n{ph}",
        method = method,
        uri = canonical_uri,
        host = host,
        ph = payload_hash,
        ad = amz_date,
        sh = signed_headers,
    );
    let scope = format!("{date_stamp}/{REGION}/{SERVICE}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, REGION.as_bytes());
    let k_service = hmac_sha256(&k_region, SERVICE.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    format!(
        "AWS4-HMAC-SHA256 Credential={ak}/{scope}, SignedHeaders={sh}, Signature={sig}",
        ak = access_key,
        scope = scope,
        sh = signed_headers,
        sig = signature,
    )
}

/// 从 endpoint 提取 host[:port]（签名 Host 头必须与实际发送一致）。
fn authority_of(endpoint: &str) -> String {
    let s = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    s.trim_end_matches('/').to_string()
}

impl MinioArtifactStore {
    /// 构造并确保 bucket 存在（PUT bucket 幂等：409 视为已存在）。
    pub async fn connect(cfg: S3Config) -> ForgeResult<Self> {
        cfg.validate()?;
        let store = Self {
            http: reqwest::Client::new(),
            cfg,
        };
        store.ensure_bucket().await?;
        Ok(store)
    }

    fn signed_request(
        &self,
        method: &str,
        key: Option<&str>,
        body: &[u8],
    ) -> (reqwest::Method, String, reqwest::header::HeaderMap, Vec<u8>) {
        let host = authority_of(&self.cfg.endpoint);
        let canonical_uri = match key {
            Some(k) => format!("/{}/{}", self.cfg.bucket, k),
            None => format!("/{}/", self.cfg.bucket),
        };
        let payload_hash = sha256_hex(body);
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let auth = sigv4_auth_header(
            method, &canonical_uri, &host, &payload_hash, &amz_date, &date_stamp,
            &self.cfg.access_key, &self.cfg.secret_key,
        );

        let url = format!("{}{}", self.cfg.endpoint.trim_end_matches('/'), canonical_uri);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-amz-content-sha256", payload_hash.parse().unwrap());
        headers.insert("x-amz-date", amz_date.parse().unwrap());
        headers.insert(reqwest::header::AUTHORIZATION, auth.parse().unwrap());

        let m = match method {
            "PUT" => reqwest::Method::PUT,
            "HEAD" => reqwest::Method::HEAD,
            _ => reqwest::Method::GET,
        };
        (m, url, headers, body.to_vec())
    }

    async fn exec(
        &self,
        method: &str,
        key: Option<&str>,
        body: &[u8],
    ) -> ForgeResult<reqwest::Response> {
        let (m, url, headers, body) = self.signed_request(method, key, body);
        let resp = self
            .http
            .request(m, &url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| ForgeError::InvalidState(format!("s3 transport: {e}")))?;
        Ok(resp)
    }

    /// 建 bucket；409(BucketAlreadyOwnedByYou/已存在) 视为成功。
    pub async fn ensure_bucket(&self) -> ForgeResult<()> {
        let resp = self.exec("PUT", None, b"").await?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            Ok(())
        } else {
            Err(ForgeError::InvalidState(format!(
                "s3 create bucket failed: {} {}",
                status.as_u16(),
                self.cfg.bucket
            )))
        }
    }
}

#[async_trait]
impl ArtifactStore for MinioArtifactStore {
    async fn put(
        &self,
        name: String,
        kind: ArtifactKind,
        content: Vec<u8>,
        meta: serde_json::Value,
    ) -> ForgeResult<Artifact> {
        let id = ArtifactId::new_artifact_id();
        let checksum = sha256_hex(&content);
        let created_at = Utc::now();

        // 额外元数据放 x-amz-meta-*；注意本方法签名走 signed_request 统一路径，
        // meta 头不参与签名（未列入 SignedHeaders），S3 允许。
        let (m, url, mut headers, body) =
            self.signed_request("PUT", Some(id.as_ref()), &content);
        headers.insert("content-type", "application/octet-stream".parse().unwrap());
        headers.insert("x-amz-meta-name", name.parse().unwrap());
        headers.insert("x-amz-meta-kind", crate::enc(&kind).parse().unwrap());
        headers.insert("x-amz-meta-checksum", checksum.parse().unwrap());
        headers.insert("x-amz-meta-created", created_at.to_rfc3339().parse().unwrap());
        headers.insert("x-amz-meta-json", serde_json::to_string(&meta).unwrap().parse().unwrap());

        let resp = self
            .http
            .request(m, &url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| ForgeError::InvalidState(format!("s3 transport: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ForgeError::InvalidState(format!(
                "s3 put failed: {} {}",
                status.as_u16(),
                text.chars().take(200).collect::<String>()
            )));
        }

        Ok(Artifact {
            id,
            name,
            kind,
            checksum_sha256: checksum,
            size_bytes: content.len() as u64,
            created_at,
            meta,
        })
    }

    async fn get_meta(&self, id: &ArtifactId) -> ForgeResult<Artifact> {
        let resp = self.exec("HEAD", Some(id.as_ref()), b"").await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(ForgeError::NotFound(format!("artifact: {id}")));
        }
        if !status.is_success() {
            return Err(ForgeError::InvalidState(format!(
                "s3 head failed: {}",
                status.as_u16()
            )));
        }
        let h = resp.headers();
        let get = |k: &str| -> ForgeResult<String> {
            h.get(k)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .ok_or_else(|| ForgeError::InvalidState(format!("s3 meta missing: {k}")))
        };
        Ok(Artifact {
            id: id.clone(),
            name: get("x-amz-meta-name")?,
            kind: crate::dec(&get("x-amz-meta-kind")?)?,
            checksum_sha256: get("x-amz-meta-checksum")?,
            size_bytes: get("content-length")?.parse().map_err(|_| {
                ForgeError::InvalidState("s3 meta: bad content-length".into())
            })?,
            created_at: chrono::DateTime::parse_from_rfc3339(&get("x-amz-meta-created")?)
                .map_err(|e| ForgeError::InvalidState(format!("s3 meta: bad created: {e}")))?
                .with_timezone(&Utc),
            meta: serde_json::from_str(&get("x-amz-meta-json")?)
                .map_err(|e| ForgeError::InvalidState(format!("s3 meta: bad json: {e}")))?,
        })
    }

    async fn read(&self, id: &ArtifactId) -> ForgeResult<Vec<u8>> {
        let resp = self.exec("GET", Some(id.as_ref()), b"").await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(ForgeError::NotFound(format!("artifact: {id}")));
        }
        if !status.is_success() {
            return Err(ForgeError::InvalidState(format!(
                "s3 get failed: {}",
                status.as_u16()
            )));
        }
        Ok(resp.bytes().await.map_err(|e| {
            ForgeError::InvalidState(format!("s3 read body: {e}"))
        })?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_deterministic_and_wellformed() {
        let a = sigv4_auth_header(
            "PUT", "/bucket/key", "localhost:19000",
            "abc123", "20260823T000000Z", "20260823", "AK", "SK",
        );
        let b = sigv4_auth_header(
            "PUT", "/bucket/key", "localhost:19000",
            "abc123", "20260823T000000Z", "20260823", "AK", "SK",
        );
        assert_eq!(a, b, "same inputs must produce identical signature");
        assert!(a.starts_with("AWS4-HMAC-SHA256 Credential=AK/20260823/us-east-1/s3/aws4_request"));
        assert!(a.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(a.contains(", Signature="));
    }

    #[test]
    fn different_payload_changes_signature() {
        let a = {
        let empty = sha256_hex(b"");
        sigv4_auth_header("GET", "/b/k", "h", &empty, "D", "D", "ak", "sk")
    };
        let b = sigv4_auth_header("GET", "/b/k", "h", "deadbeef", "D", "D", "ak", "sk");
        assert_ne!(a, b);
    }

    #[test]
    fn authority_extraction() {
        assert_eq!(authority_of("http://localhost:19000"), "localhost:19000");
        assert_eq!(authority_of("http://host/"), "host");
        assert_eq!(authority_of("host:9000"), "host:9000");
    }
}
