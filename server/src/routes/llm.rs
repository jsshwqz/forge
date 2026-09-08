//! 大模型运行时配置与页面快速切换接口
//!
//! - GET  /api/llm/config - 获取当前配置视图与厂商预设
//! - POST /api/llm/config - 热更新大模型配置（并可选持久化至 forge.env）
//! - POST /api/llm/test   - 在线连通性与模型探测测试

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::AppState;

/// 运行时大模型配置
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmRuntimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub low_model: Option<String>,
}

impl LlmRuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("FORGE_LLM_BASE_URL").unwrap_or_default(),
            api_key: std::env::var("FORGE_LLM_API_KEY").unwrap_or_default(),
            model: std::env::var("FORGE_TIER_HIGH_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "deepseek-chat".to_string()),
            low_model: std::env::var("FORGE_TIER_LOW_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        }
    }
}

/// 预设大模型供应商定义
#[derive(Clone, Debug, Serialize)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub default_low_model: Option<&'static str>,
    pub key_hint: &'static str,
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek (深度求索 - 推荐)",
        base_url: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        default_low_model: Some("deepseek-chat"),
        key_hint: "sk-...",
    },
    ProviderPreset {
        id: "dashscope",
        name: "阿里云通义千问 (DashScope)",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen-plus",
        default_low_model: Some("qwen2.5-coder-32b-instruct"),
        key_hint: "sk-...",
    },
    ProviderPreset {
        id: "ollama",
        name: "本地 Ollama (免密钥 / 离线)",
        base_url: "http://localhost:11434/v1",
        default_model: "qwen2.5-coder:7b",
        default_low_model: Some("qwen2.5-coder:7b"),
        key_hint: "ollama (任意非空字符即可)",
    },
    ProviderPreset {
        id: "sensenova",
        name: "商汤 SenseNova",
        base_url: "https://token.sensenova.cn/v1",
        default_model: "sensenova-6.8-flash-lite",
        default_low_model: Some("sensenova-6.8-flash-lite"),
        key_hint: "sk-...",
    },
    ProviderPreset {
        id: "openai",
        name: "OpenAI 官方",
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-4o",
        default_low_model: Some("gpt-4o-mini"),
        key_hint: "sk-proj-...",
    },
    ProviderPreset {
        id: "custom",
        name: "自定义兼容端点",
        base_url: "",
        default_model: "",
        default_low_model: None,
        key_hint: "根据服务端点要求输入",
    },
];

/// 脱敏 Key
pub fn mask_api_key(key: &str) -> String {
    let t = key.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.len() <= 8 {
        return "********".to_string();
    }
    let prefix = &t[..std::cmp::min(6, t.len())];
    let suffix = &t[t.len().saturating_sub(4)..];
    format!("{prefix}****{suffix}")
}

/// GET /api/llm/config 响应体
#[derive(Serialize)]
pub struct LlmConfigResponse {
    pub base_url: String,
    pub model: String,
    pub low_model: Option<String>,
    pub has_key: bool,
    pub key_masked: String,
    pub presets: &'static [ProviderPreset],
}

/// GET /api/llm/config
pub async fn get_llm_config(State(st): State<AppState>) -> Json<LlmConfigResponse> {
    let cfg = st.llm_config.read().await;
    let has_key = !cfg.api_key.trim().is_empty();
    let key_masked = mask_api_key(&cfg.api_key);
    Json(LlmConfigResponse {
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
        low_model: cfg.low_model.clone(),
        has_key,
        key_masked,
        presets: PRESETS,
    })
}

/// POST /api/llm/config 请求体
#[derive(Deserialize)]
pub struct UpdateLlmConfigRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub low_model: Option<String>,
    #[serde(default = "default_true")]
    pub persist_to_env: bool,
}

fn default_true() -> bool {
    true
}

/// POST /api/llm/config
pub async fn update_llm_config(
    State(st): State<AppState>,
    Json(req): Json<UpdateLlmConfigRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut cfg = st.llm_config.write().await;

    let base_url = req.base_url.trim().to_string();
    let model = req.model.trim().to_string();
    let low_model = req
        .low_model
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    cfg.base_url = base_url.clone();
    cfg.model = model.clone();
    cfg.low_model = low_model.clone();

    // 如果提供了非脱敏的新 Key，则更新；否则保留原有 Key
    if let Some(new_key) = req.api_key {
        let trimmed = new_key.trim();
        if !trimmed.is_empty() && !trimmed.contains("****") {
            cfg.api_key = trimmed.to_string();
        }
    }

    // 同步进程环境变量（确保任意直接读 env 的子模块同步生效）
    std::env::set_var("FORGE_LLM_BASE_URL", &cfg.base_url);
    std::env::set_var("FORGE_LLM_API_KEY", &cfg.api_key);
    std::env::set_var("FORGE_TIER_HIGH_MODEL", &cfg.model);
    if let Some(ref lm) = cfg.low_model {
        std::env::set_var("FORGE_TIER_LOW_MODEL", lm);
    } else {
        std::env::remove_var("FORGE_TIER_LOW_MODEL");
    }

    // 持久化到 deploy/local/forge.env
    let persisted = if req.persist_to_env {
        persist_env_file(&cfg.base_url, &cfg.api_key, &cfg.model, cfg.low_model.as_deref())
    } else {
        false
    };

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "大模型配置已更新并即刻热生效（无需重启服务）",
        "persisted": persisted,
        "base_url": cfg.base_url,
        "model": cfg.model,
        "low_model": cfg.low_model,
        "key_masked": mask_api_key(&cfg.api_key),
    })))
}

/// POST /api/llm/test 请求体
#[derive(Deserialize)]
pub struct TestLlmRequest {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// POST /api/llm/test - 在线连通性与模型探测
pub async fn test_llm_connection(
    State(st): State<AppState>,
    Json(req): Json<TestLlmRequest>,
) -> Json<serde_json::Value> {
    let (base_url, api_key, model) = {
        let cfg = st.llm_config.read().await;
        let b = req
            .base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cfg.base_url.clone());
        let mut k = req
            .api_key
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.contains("****"))
            .unwrap_or_else(|| cfg.api_key.clone());
        if k.is_empty() {
            k = cfg.api_key.clone();
        }
        let m = req
            .model
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cfg.model.clone());
        (b, k, m)
    };

    if base_url.is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "error": "Base URL 为空，请先配置服务地址"
        }));
    }

    let client = forge_api::LlmClient::new(base_url.clone(), api_key);
    let start = std::time::Instant::now();

    // 优先探测 list_models
    match forge_api::LlmBackend::list_models(&client).await {
        Ok(models) => {
            let elapsed = start.elapsed().as_millis();
            let sample: Vec<String> = models.into_iter().take(10).collect();
            Json(serde_json::json!({
                "ok": true,
                "latency_ms": elapsed,
                "message": format!("连通成功！检测到可用模型数量: {}", sample.len()),
                "models_sample": sample,
                "target_model": model,
            }))
        }
        Err(e_models) => {
            // 若供应商不支持 /models，回退使用 chat ping 测试单轮调用
            let ping_msg = vec![forge_api::ChatMessage::user("Hi, reply with pong.")];
            match forge_api::LlmBackend::chat(&client, &model, &ping_msg).await {
                Ok(reply) => {
                    let elapsed = start.elapsed().as_millis();
                    Json(serde_json::json!({
                        "ok": true,
                        "latency_ms": elapsed,
                        "message": "连通成功！单轮推理响应正常",
                        "reply": reply.chars().take(80).collect::<String>(),
                        "target_model": model,
                    }))
                }
                Err(e_chat) => {
                    let elapsed = start.elapsed().as_millis();
                    Json(serde_json::json!({
                        "ok": false,
                        "latency_ms": elapsed,
                        "error": format!("连通失败：/models 报错 ({e_models})；/chat 报错 ({e_chat})"),
                        "target_model": model,
                    }))
                }
            }
        }
    }
}

/// 寻找并更新 deploy/local/forge.env
fn persist_env_file(base_url: &str, api_key: &str, model: &str, low_model: Option<&str>) -> bool {
    let candidate_paths = [
        PathBuf::from("deploy/local/forge.env"),
        PathBuf::from("aion-forge/deploy/local/forge.env"),
        PathBuf::from("../deploy/local/forge.env"),
        PathBuf::from("D:/test/aionui/新forge/aion-forge/deploy/local/forge.env"),
    ];

    let target_path = candidate_paths.into_iter().find(|p| p.exists());
    let Some(path) = target_path else {
        return false;
    };

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|s| s.to_string()).collect();

    let mut set_base = false;
    let mut set_key = false;
    let mut set_high = false;
    let mut set_low = false;

    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.starts_with("FORGE_LLM_BASE_URL=") {
            *line = format!("FORGE_LLM_BASE_URL={base_url}");
            set_base = true;
        } else if trimmed.starts_with("FORGE_LLM_API_KEY=") {
            *line = format!("FORGE_LLM_API_KEY={api_key}");
            set_key = true;
        } else if trimmed.starts_with("FORGE_TIER_HIGH_MODEL=") {
            *line = format!("FORGE_TIER_HIGH_MODEL={model}");
            set_high = true;
        } else if trimmed.starts_with("FORGE_TIER_LOW_MODEL=") {
            if let Some(lm) = low_model {
                *line = format!("FORGE_TIER_LOW_MODEL={lm}");
                set_low = true;
            }
        }
    }

    if !set_base {
        lines.push(format!("FORGE_LLM_BASE_URL={base_url}"));
    }
    if !set_key {
        lines.push(format!("FORGE_LLM_API_KEY={api_key}"));
    }
    if !set_high {
        lines.push(format!("FORGE_TIER_HIGH_MODEL={model}"));
    }
    if let Some(lm) = low_model {
        if !set_low {
            lines.push(format!("FORGE_TIER_LOW_MODEL={lm}"));
        }
    }

    let updated_content = lines.join("\n") + "\n";
    std::fs::write(&path, updated_content).is_ok()
}
