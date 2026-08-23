use forge_api::{LlmBackend, LlmClient, ChatMessage};

#[tokio::test]
async fn probe_official_models() {
    let Ok(base) = std::env::var("FORGE_LLM_BASE_URL") else { eprintln!("[skip]"); return; };
    let key = std::env::var("FORGE_LLM_API_KEY").unwrap();
    let c = LlmClient::new(base, key);
    for m in ["sensenova-6.8-flash-lite", "sensenova-6.7-flash-lite"] {
        match c.chat_raw(m, &[ChatMessage::user("Reply with exactly: OK")]).await {
            Ok(v) => println!("[{m}] RAW → {}", serde_json::to_string(&v).unwrap_or_default()),
            Err(e) => println!("[{m}] ❌ → {}", e.to_string().chars().take(110).collect::<String>()),
        }
    }
}
