//! IMPROVE-9 集成测试：`forge_plan_execute` MCP 工具。
//!
//! 覆盖测试矩阵第 4 章 #1-#9（#10/#11 为 crate 内单测，分别在
//! `planning/planner/src/prefab.rs` 和 `planning/dag/src/validate.rs`）。
//!
//! 全部 temp 目录隔离，不触碰真实文件系统。

#![cfg(feature = "server-bin")]

use forge_mcp::client::McpClient;
use forge_mcp::config::McpServerConfig;
use std::collections::HashMap;

/// 构建 forge-mcp-server 的 McpServerConfig。
fn plan_exec_server_config() -> McpServerConfig {
    let bin = env!("CARGO_BIN_EXE_forge-mcp-server");
    let mut env = HashMap::new();
    env.insert(
        "FORGE_TOOLS_BUILTIN".to_string(),
        "forge_task_create,forge_task_get,forge_task_list,forge_orchestrate,forge_plan_execute".to_string(),
    );
    let ws = std::env::temp_dir().join(format!("forge-mcp-plan-exec-test-{}", std::process::id()));
    env.insert("FORGE_WORKSPACE".to_string(), ws.to_string_lossy().to_string());
    McpServerConfig {
        name: "forge".into(),
        command: bin.into(),
        args: vec![],
        env,
    }
}

/// 创建任务并返回 task_id。
async fn create_task(client: &mut McpClient, name: &str, acceptance: serde_json::Value) -> String {
    let create = client
        .call_tool(
            "forge_task_create",
            serde_json::json!({ "name": name, "acceptance": acceptance }),
        )
        .await
        .unwrap();
    let text = create["content"][0]["text"].as_str().unwrap();
    serde_json::from_str::<serde_json::Value>(text).unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 调用 forge_plan_execute 并解析返回的 JSON。
async fn call_plan_execute(
    client: &mut McpClient,
    task_id: &str,
    plan: serde_json::Value,
) -> Result<serde_json::Value, forge_core::ForgeError> {
    let result = client
        .call_tool(
            "forge_plan_execute",
            serde_json::json!({ "task_id": task_id, "plan": plan }),
        )
        .await?;
    let text = result["content"][0]["text"].as_str().unwrap();
    Ok(serde_json::from_str(text).unwrap())
}

/// 构造一个 CallCapability step。
fn step(id: &str, title: &str, deps: Vec<&str>, capability: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "depends_on": deps,
        "action": { "CallCapability": { "capability": capability, "input": input } }
    })
}

/// 构造一个 HumanApproval step。
fn human_step(id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "depends_on": [],
        "action": { "HumanApproval": "approve?" }
    })
}

fn plan_with_steps(steps: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({ "steps": steps })
}

// ── #1: happy path with step ref ──

#[tokio::test]
async fn plan_execute_happy_path_with_step_ref() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "plan exec happy path",
        serde_json::json!([{
            "id": "AC-1",
            "description": "file written",
            "check": { "FileContains": { "path": "out.txt", "needle": "hello" } }
        }]),
    )
    .await;

    let plan = serde_json::json!({
        "id": "plan_forged_by_client",
        "steps": [
            step("s1", "write file", vec![], "write_file",
                 serde_json::json!({ "path": "out.txt", "content": "hello" })),
            step("s2", "read file", vec!["s1"], "read_file",
                 serde_json::json!({ "path": "$s1.output.written" })),
        ]
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .expect("plan_execute should succeed");

    assert_eq!(result["final_status"], "Completed");
    // plan.id is server-generated (not a client-forged value)
    let plan_id = result["plan"]["id"].as_str().unwrap();
    assert!(plan_id.starts_with("plan_"), "server-generated plan id: {plan_id}");
    assert_ne!(plan_id, "plan_forged_by_client");

    client.shutdown().await.unwrap();
}

// ── #2: rejects unknown capability ──

#[tokio::test]
async fn plan_execute_rejects_unknown_capability() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "unknown cap",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan = plan_with_steps(vec![
        step("s1", "bad cap", vec![], "nonexistent_tool", serde_json::json!({})),
    ]);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("unknown capability"), "got: {msg}");
    assert!(msg.contains("echo"), "should list available: {msg}");

    client.shutdown().await.unwrap();
}

// ── #3: rejects cycle ──

#[tokio::test]
async fn plan_execute_rejects_cycle() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "cycle",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan = plan_with_steps(vec![
        step("s1", "dep s2", vec!["s2"], "echo", serde_json::json!({ "input": "a" })),
        step("s2", "dep s1", vec!["s1"], "echo", serde_json::json!({ "input": "b" })),
    ]);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("cycle"), "got: {msg}");

    client.shutdown().await.unwrap();
}

// ── #4: rejects dangling dependency ──

#[tokio::test]
async fn plan_execute_rejects_dangling_dependency() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "dangling",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan = plan_with_steps(vec![
        step("s1", "dep ghost", vec!["nonexistent"], "echo", serde_json::json!({})),
    ]);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("dependency missing"), "got: {msg}");

    client.shutdown().await.unwrap();
}

// ── #5: rejects duplicate step ids ──

#[tokio::test]
async fn plan_execute_rejects_duplicate_step_ids() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "dup ids",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan = plan_with_steps(vec![
        step("s1", "first", vec![], "echo", serde_json::json!({})),
        step("s1", "second", vec![], "echo", serde_json::json!({})),
    ]);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("duplicate step id"), "got: {msg}");

    client.shutdown().await.unwrap();
}

// ── #6: rejects HumanApproval ──

#[tokio::test]
async fn plan_execute_rejects_human_approval() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "human approval",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan = plan_with_steps(vec![human_step("s1", "approve me")]);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("HumanApproval not supported"), "got: {msg}");

    client.shutdown().await.unwrap();
}

// ── #7: rejects oversized plan ──

#[tokio::test]
async fn plan_execute_rejects_oversized_plan() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "oversized",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let steps: Vec<serde_json::Value> = (0..33)
        .map(|i| step(&format!("s{i}"), "step", vec![], "echo", serde_json::json!({})))
        .collect();
    let plan = plan_with_steps(steps);

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("too many steps"), "got: {msg}");

    client.shutdown().await.unwrap();
}

// ── #8: failure escalates to agent ──

#[tokio::test]
async fn plan_execute_failure_escalates_to_agent() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "failure escalate",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    // read_file on a non-existent file → tool fails → recovery retry → still fail → escalate
    // (no replanner → escalate)
    let plan = plan_with_steps(vec![
        step("s1", "fail step", vec![], "read_file",
             serde_json::json!({ "path": "this_file_does_not_exist.txt" })),
    ]);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .expect("should return report even on failure");

    assert_eq!(result["final_status"], "Failed");
    assert_eq!(result["escalated_to_human"], true);
    assert_eq!(result["replans_used"], 0);

    client.shutdown().await.unwrap();
}

// ── #9: gate governs completion ──

#[tokio::test]
async fn plan_execute_gate_governs_completion() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    let task_id = create_task(
        &mut client,
        "gate governs",
        serde_json::json!([{
            "id": "AC-1",
            "description": "file must contain specific content",
            "check": { "FileContains": { "path": "out.txt", "needle": "expected_content" } }
        }]),
    )
    .await;

    // Steps succeed (write "wrong" content), but acceptance requires "expected_content"
    let plan = plan_with_steps(vec![
        step("s1", "write wrong content", vec![], "write_file",
             serde_json::json!({ "path": "out.txt", "content": "wrong_content" })),
    ]);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task_id, plan),
    )
    .await
    .expect("timeout")
    .expect("should return report");

    // Steps executed but gate fails → final_status = Failed (验收权不外包)
    assert_eq!(result["final_status"], "Failed");

    client.shutdown().await.unwrap();
}

// ── #10: workspace_task resume preserves prior step outputs ($s1.output.*) ──

#[tokio::test]
async fn plan_execute_workspace_resume_preserves_step_outputs() {
    let cfg = plan_exec_server_config();
    let mut client = McpClient::connect(&cfg).await.unwrap();

    // 第 1 次调用：s1 写入 resumed.txt 成功，s2 读取不存在文件失败
    let task1_id = create_task(
        &mut client,
        "partial run",
        serde_json::json!([{
            "id": "AC-1", "description": "noop", "check": { "Command": "exit 0" }
        }]),
    )
    .await;

    let plan1 = plan_with_steps(vec![
        step(
            "s1",
            "write file in run 1",
            vec![],
            "write_file",
            serde_json::json!({ "path": "resumed.txt", "content": "persisted_across_runs" }),
        ),
        step(
            "s2",
            "fail on missing file",
            vec!["s1"],
            "read_file",
            serde_json::json!({ "path": "missing_in_run1.txt" }),
        ),
    ]);

    let res1 = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call_plan_execute(&mut client, &task1_id, plan1),
    )
    .await
    .expect("timeout")
    .expect("run 1 report");
    assert_eq!(res1["final_status"], "Failed");

    // 第 2 次调用：新建修复任务并传入 workspace_task = task1_id，
    // 新计划只有 s3（不再声明 s1 依赖，但 input 直接引用第 1 次已成功步骤的 $s1.output.written）
    let task2_id = create_task(
        &mut client,
        "resume run",
        serde_json::json!([{
            "id": "AC-1",
            "description": "resumed.txt exists from run 1",
            "check": { "FileContains": { "path": "resumed.txt", "needle": "persisted_across_runs" } }
        }]),
    )
    .await;

    let plan2 = plan_with_steps(vec![
        step(
            "s3",
            "read file via $s1.output.written from previous run",
            vec![],
            "read_file",
            serde_json::json!({ "path": "$s1.output.written" }),
        ),
    ]);

    let res2_raw = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        client.call_tool(
            "forge_plan_execute",
            serde_json::json!({
                "task_id": task2_id,
                "plan": plan2,
                "workspace_task": task1_id,
            }),
        ),
    )
    .await
    .expect("timeout")
    .expect("run 2 report");
    let text2 = res2_raw["content"][0]["text"].as_str().expect("text content");
    let res2: serde_json::Value = serde_json::from_str(text2).expect("valid json");

    assert_eq!(res2["final_status"], "Completed");
    assert_eq!(res2["gate"]["passed"], true);

    client.shutdown().await.unwrap();
}

