//! MCP-002 集成测试：编排能力（plan→execute→verify→gate）经 MCP stdio 全链路。
//!
//! 覆盖：
//! - 白名单点名注册编排工具（缺省不注册，保持 MCP-001 零回归）
//! - forge_task_create → forge_orchestrate 全链路（断言终态 Completed + gate.passed）
//! - MCP-002-B 服务端调用闸 FORGE_MCP_ALLOWLIST：放行/拒绝
//!
//! 全部 temp 目录隔离，不触碰真实文件系统。

#![cfg(feature = "server-bin")]

use forge_mcp::client::McpClient;
use forge_mcp::config::McpServerConfig;
use std::collections::HashMap;

/// 构建 forge-mcp-server 的 McpServerConfig（编排工具白名单点名注册）。
fn orch_server_config(extra_env: Vec<(&str, &str)>) -> McpServerConfig {
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let mut env = HashMap::new();
    // 编排工具白名单 + 隔离工作区
    env.insert(
        "FORGE_TOOLS_BUILTIN".to_string(),
        "forge_task_create,forge_task_get,forge_task_list,forge_orchestrate".to_string(),
    );
    let ws = std::env::temp_dir().join(format!("forge-mcp-orch-test-{}", std::process::id()));
    env.insert("FORGE_WORKSPACE".to_string(), ws.to_string_lossy().to_string());
    for (k, v) in extra_env {
        env.insert(k.to_string(), v.to_string());
    }
    McpServerConfig {
        name: "forge".into(),
        command: bin.into(),
        args: vec![],
        env,
    }
}

#[tokio::test]
async fn orchestrate_tools_registered_when_whitelisted() {
    let cfg = orch_server_config(vec![]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let tools = client.list_tools().await.unwrap();
    // 5 base + 4 编排 = 9（编排工具经 FORGE_TOOLS_BUILTIN 点名注册）
    assert_eq!(tools.len(), 9, "5 base + 4 orchestrate = 9");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expect in [
        "forge_task_create",
        "forge_task_get",
        "forge_task_list",
        "forge_orchestrate",
    ] {
        assert!(names.contains(&expect), "missing {expect}");
    }

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn orchestrate_tools_not_registered_by_default() {
    // 不设 FORGE_TOOLS_BUILTIN → 编排工具不注册（MCP-001 零回归）
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let mut env = HashMap::new();
    let ws = std::env::temp_dir().join(format!("forge-mcp-def-test-{}", std::process::id()));
    env.insert("FORGE_WORKSPACE".to_string(), ws.to_string_lossy().to_string());
    let cfg = McpServerConfig {
        name: "forge".into(),
        command: bin.into(),
        args: vec![],
        env,
    };
    let mut client = McpClient::connect(&cfg).await.unwrap();
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 5, "default stays 5 base tools");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn task_create_get_list_via_mcp() {
    let cfg = orch_server_config(vec![]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    // create
    let create = client
        .call_tool(
            "forge_task_create",
            serde_json::json!({
                "name": "mcp roundtrip",
                "constraints": [],
                "acceptance": [{
                    "id": "AC-1",
                    "description": "noop ok",
                    "check": { "Command": "true" }
                }]
            }),
        )
        .await
        .unwrap();
    let text = create["content"][0]["text"].as_str().unwrap();
    let created: serde_json::Value = serde_json::from_str(text).unwrap();
    let task_id = created["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("task_"));

    // get
    let get = client
        .call_tool("forge_task_get", serde_json::json!({ "task_id": task_id }))
        .await
        .unwrap();
    let get_text = get["content"][0]["text"].as_str().unwrap();
    assert!(get_text.contains("mcp roundtrip"));

    // list
    let list = client.call_tool("forge_task_list", serde_json::json!({})).await.unwrap();
    let list_text = list["content"][0]["text"].as_str().unwrap();
    assert!(list_text.contains(&task_id));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn orchestrate_end_to_end_via_mcp() {
    let cfg = orch_server_config(vec![]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let create = client
        .call_tool(
            "forge_task_create",
            serde_json::json!({
                "name": "orchestrate via mcp",
                "acceptance": [{
                    "id": "AC-1",
                    "description": "sh true passes",
                    "check": { "Command": "true" }
                }]
            }),
        )
        .await
        .unwrap();
    let text = create["content"][0]["text"].as_str().unwrap();
    let task_id = serde_json::from_str::<serde_json::Value>(text)
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();

    let report = client
        .call_tool("forge_orchestrate", serde_json::json!({ "task_id": task_id }))
        .await
        .unwrap();
    let report_text = report["content"][0]["text"].as_str().unwrap();
    let rv: serde_json::Value = serde_json::from_str(report_text).unwrap();

    assert_eq!(rv["final_status"], "Completed");
    assert_eq!(rv["gate"]["passed"], true);
    assert!(!rv["plan_versions"].as_array().unwrap().is_empty());

    client.shutdown().await.unwrap();
}

// ── MCP-002-B：服务端调用闸 FORGE_MCP_ALLOWLIST ──

#[tokio::test]
async fn allowlist_rejects_outside_whitelist() {
    // 只允许 echo；forge_task_create 不在白名单 → tools/call 拒绝
    let cfg = orch_server_config(vec![("FORGE_MCP_ALLOWLIST", "echo")]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    // 白名单内放行
    let echo = client
        .call_tool("echo", serde_json::json!({ "input": "hi" }))
        .await
        .unwrap();
    assert!(echo["content"][0]["text"].as_str().unwrap().contains("hi"));

    // 白名单外拒绝 → McpClient 把 error 转成 ForgeError(InvalidState)
    let err = client
        .call_tool("forge_task_create", serde_json::json!({ "name": "x" }))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("allowlist rejected"), "got: {msg}");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn allowlist_unset_allows_all() {
    // 不设 FORGE_MCP_ALLOWLIST → 全部放行（与 MCP-001 兼容）
    let cfg = orch_server_config(vec![]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let create = client
        .call_tool(
            "forge_task_create",
            serde_json::json!({ "name": "no allowlist" }),
        )
        .await;
    assert!(create.is_ok(), "unset allowlist must allow: {create:?}");

    client.shutdown().await.unwrap();
}

// ── MCP-003：验收驱动规划 + 工作区对齐 + 台账工具 ──

#[tokio::test]
async fn practice1_file_exists_orchestrate_completes() {
    // Practice-1 回归：任务"写 out.txt"+ FileExists 验收 → 必须 Completed 且文件真实存在
    let cfg = orch_server_config(vec![]);
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let create = client
        .call_tool(
            "forge_task_create",
            serde_json::json!({
                "name": "create out.txt via orchestrate",
                "acceptance": [{
                    "id": "AC-1",
                    "description": "out exists",
                    "check": { "FileExists": "out.txt" }
                }]
            }),
        )
        .await
        .unwrap();
    let task_id = serde_json::from_str::<serde_json::Value>(
        create["content"][0]["text"].as_str().unwrap()
    ).unwrap()["task_id"].as_str().unwrap().to_string();

    let report = client
        .call_tool("forge_orchestrate", serde_json::json!({ "task_id": task_id }))
        .await
        .unwrap();
    let rv: serde_json::Value =
        serde_json::from_str(report["content"][0]["text"].as_str().unwrap()).unwrap();

    assert_eq!(rv["final_status"], "Completed", "file task must complete: {rv}");
    assert_eq!(rv["gate"]["passed"], true);

    // 验证文件真实落盘：workdir 来自报告
    let workdir = rv["workdir"].as_str().unwrap();
    let file = std::path::Path::new(workdir).join("out.txt");
    assert!(file.exists(), "out.txt must exist in workdir");
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("create out.txt via orchestrate"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn worklog_add_and_export_via_mcp() {
    // 台账工具：追加一条记录并导出（FORGE_PROJECT_ROOT 指向仓库根）
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let mut env = HashMap::new();
    env.insert("FORGE_TOOLS_BUILTIN".to_string(), "forge_worklog_add,forge_worklog_show".to_string());
    // 仓库根：从 CARGO_MANIFEST_DIR 向上找 AI_WORKFLOW.md
    let mut root = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let root_path = std::path::PathBuf::from(&root);
    if !root_path.join("AI_WORKFLOW.md").exists() {
        // capability/mcp → 上两级到仓库根
        if let Some(parent) = root_path.parent().and_then(|p| p.parent()) {
            root = parent.to_string_lossy().to_string();
        }
    }
    env.insert("FORGE_PROJECT_ROOT".to_string(), root);
    let tmp = tempfile::tempdir().unwrap();
    env.insert("FORGE_WORKSPACE".to_string(), tmp.path().to_string_lossy().to_string());
    let cfg = McpServerConfig {
        name: "forge".into(),
        command: bin.into(),
        args: vec![],
        env,
    };
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let add = client
        .call_tool(
            "forge_worklog_add",
            serde_json::json!({
                "kind": "R4",
                "title": "MCP-003 integration test",
                "body": "practice test record",
                "task_id": "MCP-003"
            }),
        )
        .await
        .unwrap();
    let text = add["content"][0]["text"].as_str().unwrap();
    let rv: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(rv["record_id"].as_str().unwrap().starts_with("R4-"));

    let show = client.call_tool("forge_worklog_show", serde_json::json!({ "limit": 1 })).await.unwrap();
    assert!(show["content"][0]["text"].as_str().unwrap().contains("MCP-003 integration test"));

    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn progress_update_via_mcp() {
    // 台账工具：更新进度卡状态（在隔离副本上验证，不污染仓库真实台账）
    // 用临时目录模拟项目根，先写最小 progress.json 种子
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    std::fs::write(
        tmp.path().join("progress.json"),
        r#"[{"task_id":"DEMO-001","name":"demo task","status":"NotStarted","owner":null,"last_record":null,"commit":null}]"#,
    ).unwrap();
    // AI_WORKFLOW.md 占位，让 detect_project_root 认可
    std::fs::write(tmp.path().join("AI_WORKFLOW.md"), "# dw").unwrap();

    let mut env = HashMap::new();
    env.insert("FORGE_TOOLS_BUILTIN".to_string(), "forge_progress_update".to_string());
    env.insert("FORGE_PROJECT_ROOT".to_string(), root);
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    env.insert("FORGE_WORKSPACE".to_string(), ws.to_string_lossy().to_string());
    let cfg = McpServerConfig { name: "forge".into(), command: bin.into(), args: vec![], env };
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let upd = client
        .call_tool(
            "forge_progress_update",
            serde_json::json!({"task_id":"DEMO-001","status":"Completed","owner":"GLM","commit":"abc123"}),
        )
        .await
        .unwrap();
    assert!(upd["content"][0]["text"].as_str().unwrap().contains("ok\":true"));

    // 验证落盘
    let raw = std::fs::read_to_string(tmp.path().join("progress.json")).unwrap();
    assert!(raw.contains("Completed") && raw.contains("abc123") && raw.contains("GLM"));

    // 不存在的任务 → 错误
    let err = client
        .call_tool("forge_progress_update", serde_json::json!({"task_id":"NOPE","status":"Wip"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("task not found"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn progress_add_then_update_via_mcp() {
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    std::fs::write(tmp.path().join("progress.json"), "[]").unwrap();
    std::fs::write(tmp.path().join("AI_WORKFLOW.md"), "# dw").unwrap();
    let mut env = HashMap::new();
    env.insert("FORGE_TOOLS_BUILTIN".to_string(), "forge_progress_add,forge_progress_update".to_string());
    env.insert("FORGE_PROJECT_ROOT".to_string(), root);
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    env.insert("FORGE_WORKSPACE".to_string(), ws.to_string_lossy().to_string());
    let cfg = McpServerConfig { name: "forge".into(), command: bin.into(), args: vec![], env };
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let add = client.call_tool("forge_progress_add", serde_json::json!({"task_id":"T-ADD-1","name":"add test"})).await.unwrap();
    assert!(add["content"][0]["text"].as_str().unwrap().contains("ok"));

    // 重复建卡 → 错误
    let dup = client.call_tool("forge_progress_add", serde_json::json!({"task_id":"T-ADD-1","name":"dup"})).await.unwrap_err();
    assert!(dup.to_string().contains("already exists"));

    let upd = client.call_tool("forge_progress_update", serde_json::json!({"task_id":"T-ADD-1","status":"Wip","owner":"me"})).await.unwrap();
    assert!(upd["content"][0]["text"].as_str().unwrap().contains("Wip"));
    client.shutdown().await.unwrap();
}

