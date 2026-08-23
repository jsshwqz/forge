//! forge-api：模型供应商客户端（OpenAI 兼容协议）+ LlmAgent 桥接（PH2-004）。
//!
//! - [`LlmClient`]：`GET /models` 自动发现 + `POST /chat/completions`
//! - [`LlmAgent`]：实现 forge_agent::Agent，把真实模型接入回合引擎（落实 B-05）
//!
//! 秘密纪律：API KEY 仅经环境变量注入，不入库不入日志。

pub mod llm_agent;

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use serde::{Deserialize, Serialize};

/// OpenAI 兼容聊天消息。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// LLM 后端抽象（便于测试注入 mock）。
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// 列举可用模型 id。
    async fn list_models(&self) -> ForgeResult<Vec<String>>;
    /// 单轮对话，返回助手文本。
    async fn chat(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String>;
}

async fn tracing_stub_wait(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// OpenAI 兼容 HTTP 客户端（商汤 SenseNova 等同协议服务通用）。
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl LlmClient {
    /// 构造客户端。base_url 形如 `https://host/v1`。
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    fn err_from_response(status: reqwest::StatusCode, body: String) -> ForgeError {
        let short: String = body.chars().take(200).collect();
        ForgeError::InvalidState(format!("llm http {}: {}", status.as_u16(), short))
    }

    async fn handle(&self, resp: reqwest::Response) -> ForgeResult<serde_json::Value> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(Self::err_from_response(status, text));
        }
        serde_json::from_str(&text)
            .map_err(|e| ForgeError::InvalidState(format!("llm bad json: {e}")))
    }

    async fn get_json(&self, path: &str) -> ForgeResult<serde_json::Value> {
        let resp = self
            .http
            .get(format!("{}/{}", self.base_url, path))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| ForgeError::InvalidState(format!("llm transport: {e}")))?;
        self.handle(resp).await
    }


    /// POST + 429 指数退避重试（最多 3 次：1.5s/3s/6s）。
    /// 商汤网关存在短窗口限流（实测同模型偶发 429，稍候即恢复）。
    async fn post_json_retry_429(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> ForgeResult<serde_json::Value> {
        let mut backoff_ms = 1500u64;
        for attempt in 1..=3 {
            let resp = self
                .http
                .post(format!("{}/{}", self.base_url, path))
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await
                .map_err(|e| ForgeError::InvalidState(format!("llm transport: {e}")))?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < 3 {
                tracing_stub_wait(backoff_ms).await;
                backoff_ms *= 2;
                continue;
            }
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(ForgeError::InvalidState(format!(
                    "llm http {}: {}",
                    status.as_u16(),
                    text.chars().take(200).collect::<String>()
                )));
            }
            return serde_json::from_str(&text)
                .map_err(|e| ForgeError::InvalidState(format!("llm bad json: {e}")));
        }
        unreachable!("retry loop always returns")
    }

    /// 从响应 JSON 提取 choices[0].message.content。
    pub(crate) fn extract_content(v: &serde_json::Value) -> ForgeResult<String> {
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                ForgeError::InvalidState("llm: missing choices[0].message.content".into())
            })
    }
}

#[async_trait]
impl LlmBackend for LlmClient {
    async fn list_models(&self) -> ForgeResult<Vec<String>> {
        let v = self.get_json("models").await?;
        let ids: Vec<String> = v
            .pointer("/data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Ok(ids)
    }

    async fn chat(&self, model: &str, messages: &[ChatMessage]) -> ForgeResult<String> {
        let body = serde_json::json!({ "model": model, "messages": messages });
        let v = self.post_json_retry_429("chat/completions", &body).await?;
        Self::extract_content(&v)
    }
}

/// 模型自动选择：按偏好子串序列依次匹配（大小写不敏感），否则排序第一个；空列表报错。
///
/// 默认偏好 `["glm", "chat"]` 来源：2026-08-23 对 SenseNova 实测——
/// deepseek-v4-flash 返回 429 insufficient_quota、sensenova-u1-fast 列表存在但 404，
/// glm-5.2 真实可用（见 WORKLOG R1-014 / R3-002）。
pub fn pick_model_with_prefs(ids: &[String], prefer: &[&str]) -> ForgeResult<String> {
    for p in prefer {
        if let Some(id) = ids.iter().find(|i| i.to_lowercase().contains(p)) {
            return Ok(id.clone());
        }
    }
    ids.iter()
        .min()
        .cloned()
        .ok_or_else(|| ForgeError::NotFound("no models available from provider".into()))
}

/// 兼容入口：仅按 `chat` 偏好。
pub fn pick_default_model(ids: &[String]) -> ForgeResult<String> {
    pick_model_with_prefs(ids, &["chat"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_chat_named_model() {
        let ids = vec![
            "SenseChat-Turbo".to_string(),
            "abc-embed".to_string(),
            "chat-pro-max".to_string(),
        ];
        assert_eq!(pick_default_model(&ids).unwrap(), "SenseChat-Turbo");
    }

    #[test]
    fn pick_falls_back_to_first_sorted() {
        let ids = vec!["b-model".to_string(), "a-model".to_string()];
        assert_eq!(pick_default_model(&ids).unwrap(), "a-model");
    }

    #[test]
    fn pick_empty_is_not_found() {
        assert!(pick_default_model(&[]).is_err());
    }

    #[test]
    fn extract_content_reads_pointer() {
        let v = serde_json::json!({
            "choices": [ { "message": { "role": "assistant", "content": "hello" } } ]
        });
        assert_eq!(LlmClient::extract_content(&v).unwrap(), "hello");
    }

    #[test]
    fn extract_content_missing_is_error() {
        assert!(LlmClient::extract_content(&serde_json::json!({"choices":[]})).is_err());
    }

    #[test]
    fn chat_message_roles() {
        assert_eq!(ChatMessage::system("s").role, "system");
        assert_eq!(ChatMessage::user("u").role, "user");
        assert_eq!(ChatMessage::assistant("a").role, "assistant");
    }
}
