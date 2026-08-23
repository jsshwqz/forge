//! B-05 终极闭环：真实模型穿过 Agent trait 的活体验证。
//!
//! 与 live.rs（裸 chat）的区别：本测试走 `LlmAgent::connect` 完整路径——
//! 模型自动发现(官方偏好) → AgentConfig 组装 → act(TurnInput) 决策映射，
//! 即生产中 TurnEngine 每回合真实调用的入口。
//!
//! 运行：FORGE_LLM_LIVE=1 + FORGE_LLM_* 三件套。

use forge_api::{LlmAgent, LlmBackend, LlmClient};
use forge_agent::{Agent, AgentAction};
use forge_core::SessionId;

fn client_or_skip() -> Option<LlmClient> {
    if std::env::var("FORGE_LLM_LIVE").as_deref() != Ok("1") {
        eprintln!("[skip] 未设 FORGE_LLM_LIVE=1");
        return None;
    }
    let (Ok(base), Ok(key)) = (
        std::env::var("FORGE_LLM_BASE_URL"),
        std::env::var("FORGE_LLM_API_KEY"),
    ) else {
        eprintln!("[skip] FORGE_LLM_* 未设置");
        return None;
    };
    Some(LlmClient::new(base, key))
}

#[tokio::test]
async fn real_model_drives_agent_trait() {
    let Some(client) = client_or_skip() else { return };

    // 1) 自动发现官方模型
    let models = client.list_models().await.unwrap();
    let picked = pick_from(&models);
    println!("选中模型: {picked}");

    // 2) AgentConfig + LlmAgent 组装
    let config = forge_agent::AgentConfig {
        id: forge_core::new_agent_id(),
        name: "live-builder".into(),
        role: forge_agent::AgentRole::Builder,
        max_turns: 3,
    };
    let agent = LlmAgent::connect(config, client, Some(picked)).await.unwrap();

    // 3) 首回合决策（无观察）
    let input = forge_agent::TurnInput {
        session_id: SessionId::new_session_id(),
        turn: 1,
        history: vec![],
        observation: None,
    };
    let action = agent.act(&input).await.unwrap();
    let text = match action {
        AgentAction::Reply(t) => t,
        other => panic!("expected Reply, got {other:?}"),
    };
    println!("首回合回复: {}", text.chars().take(150).collect::<String>());
    assert!(!text.trim().is_empty());

    // 4) 带观察的第二回合（模拟工具执行后的下一轮）
    let input2 = forge_agent::TurnInput {
        session_id: input.session_id.clone(),
        turn: 2,
        history: vec![serde_json::json!({"echo": {"step": 1}})],
        observation: Some(serde_json::json!({
            "echo": {"input": {"msg": "step-1 done"}, "status": "Success"}
        })),
    };
    let action2 = agent.act(&input2).await.unwrap();
    match action2 {
        AgentAction::Reply(t) => {
            println!("次回合回复: {}", t.chars().take(150).collect::<String>());
            assert!(!t.trim().is_empty());
        }
        other => panic!("expected Reply, got {other:?}"),
    }
}

#[tokio::test]
async fn real_model_works_toward_injected_goal() {
    let Some(client) = client_or_skip() else { return };
    let models = client.list_models().await.unwrap();
    let picked = pick_from(&models);

    let config = forge_agent::AgentConfig {
        id: forge_core::new_agent_id(),
        name: "goal-builder".into(),
        role: forge_agent::AgentRole::Builder,
        max_turns: 3,
    };
    // 关键差异：注入任务目标（R7-004 落地）
    let agent = LlmAgent::connect(config, client, Some(picked))
        .await
        .unwrap()
        .with_task_goal("创建文件 hello.txt，内容为 Hello AionForge");

    let input = forge_agent::TurnInput {
        session_id: SessionId::new_session_id(),
        turn: 1,
        history: vec![],
        observation: Some(serde_json::json!({
            "available_tool": "echo",
            "hint": "describe the command you would run"
        })),
    };
    let action = agent.act(&input).await.unwrap();
    match action {
        AgentAction::Reply(t) => {
            println!("带目标回复: {}", t.chars().take(300).collect::<String>());
            assert!(!t.trim().is_empty());
            // 软断言：回复应与目标相关（提到文件/命令/创建等关键词之一，
            // 中英文任一即可），而非再次索要目标
            let lower = t.to_lowercase();
            let relevant = ["hello.txt", "创建", "写", "echo", "file", "command", "touch"]
                .iter()
                .any(|k| lower.contains(k) || t.contains(k));
            assert!(relevant, "reply should reference the goal, got: {t}");
        }
        other => panic!("expected Reply, got {other:?}"),
    }
}

fn pick_from(ids: &[String]) -> String {
    // 与 OFFICIAL_MODEL_PREFS 相同启发式的本地复刻（避免导出测试专用依赖方向）
    for p in ["sensenova-6.8", "sensenova-6.7", "glm", "chat"] {
        if let Some(id) = ids.iter().find(|i| i.to_lowercase().contains(p)) {
            return id.clone();
        }
    }
    ids.first().cloned().expect("provider returned no models")
}

