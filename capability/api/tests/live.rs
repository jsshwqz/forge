//! PH2-004 真实调用集成测试（商汤 SenseNova）。
//!
//! 运行方式（KEY 来自 gitignored .env，勿硬编码入库）：
//! ```powershell
//! $env:FORGE_LLM_BASE_URL = "https://token.sensenova.cn/v1"
//! $env:FORGE_LLM_API_KEY  = "<your-key>"
//! cargo test -p forge-api --test live
//! ```

use forge_api::{pick_model_with_prefs, LlmBackend, LlmClient};

fn client() -> Option<LlmClient> {
    let (Ok(base), Ok(key)) = (
        std::env::var("FORGE_LLM_BASE_URL"),
        std::env::var("FORGE_LLM_API_KEY"),
    ) else {
        eprintln!("[skip] FORGE_LLM_* 未设置——跳过真实调用");
        return None;
    };
    Some(LlmClient::new(base, key))
}

#[tokio::test]
async fn live_list_models_and_auto_pick() {
    let Some(c) = client() else { return };
    let models = c.list_models().await.expect("list_models failed");
    assert!(!models.is_empty(), "provider returned no models");
    println!("可用模型数: {}", models.len());
    for m in models.iter().take(5) {
        println!("  - {m}");
    }
    let picked = pick_model_with_prefs(&models, &["glm", "chat"]).unwrap();
    println!("自动选择: {picked}");
}

#[tokio::test]
async fn live_chat_roundtrip() {
    let Some(c) = client() else { return };
    let models = c.list_models().await.unwrap();
    let model = pick_model_with_prefs(&models, &["glm", "chat"]).unwrap();

    use forge_api::ChatMessage;
    let reply = c
        .chat(
            &model,
            &[
                ChatMessage::system("You are a terse assistant."),
                ChatMessage::user("Reply with exactly: FORGE-LIVE-OK"),
            ],
        )
        .await
        .expect("chat failed");
    println!("模型回复: {}", reply.chars().take(120).collect::<String>());
    assert!(!reply.trim().is_empty(), "reply must not be empty");
}
