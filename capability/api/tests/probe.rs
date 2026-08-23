use forge_api::{LlmBackend, LlmClient, ChatMessage};

#[tokio::test]
async fn probe_models() {
    let Ok(base) = std::env::var("FORGE_LLM_BASE_URL") else { eprintln!("[skip]"); return; };
    let key = std::env::var("FORGE_LLM_API_KEY").unwrap();
    let c = LlmClient::new(base, key);
    let models = c.list_models().await.unwrap();
    println!("全部模型: {models:?}");
    for m in ["sensenova-u1-fast", "glm-5.2", "sensenova-6.7-flash-lite", "deepseek-v4-flash"] {
        match c.chat(m, &[ChatMessage::user("Reply: OK")]).await {
            Ok(t) => { println!("[{m}] 成功 → {}", t.chars().take(60).collect::<String>()); break; }
            Err(e) => println!("[{m}] 失败 → {}", e.to_string().chars().take(100).collect::<String>()),
        }
    }
}
