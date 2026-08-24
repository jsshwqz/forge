//! 完整交付演示：创建任务 → 端到端编排（计划→执行→验证→门禁）→ 报告。
//!
//! 运行方式：
//! ```bash
//! cargo run -p forge-sdk --example full_delivery
//! ```
//!
//! 演示内容：
//! 1. 创建带验收标准的任务
//! 2. 注册一个"文件写入"工具（模拟真实交付物）
//! 3. 端到端编排执行
//! 4. 打印结构化报告

use forge_core::ForgeResult;
use forge_exec::{
    PermissionLevel, PermissionPolicy, PolicyContext,
    Tool, ToolDescriptor, ToolRouter,
};
use forge_evidence::InMemoryEvidenceStore;
use forge_sdk::{ForgeSdk, Orchestrator, OrchestratorDeps};
use forge_verify::{CommandVerifier, FileVerifier};
use forge_workspace::WorkspaceManager;
use std::sync::Arc;
use std::time::Duration;

/// AllowAll 策略（demo 用；生产环境需替换为严格策略）。
struct DemoAllowAll;
impl PermissionPolicy for DemoAllowAll {
    fn check(
        &self,
        _level: PermissionLevel,
        _ctx: &PolicyContext,
    ) -> ForgeResult<()> {
        Ok(())
    }
}

/// 文件写入工具：把 input.content 写入 input.path（相对 workdir）。
struct WriteFileTool {
    descriptor: ToolDescriptor,
}

impl WriteFileTool {
    fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "write_file".into(),
                description: "Writes content to a file".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }),
                permission: PermissionLevel::WorkspaceWrite,
            },
        }
    }
}

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| forge_core::ForgeError::InvalidState("missing path".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| forge_core::ForgeError::InvalidState("missing content".into()))?;

        // 写入当前目录（orchestrator 的 workdir 由外部管理）
        let full = std::path::Path::new(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(forge_core::ForgeError::Io)?;
        }
        std::fs::write(full, content).map_err(forge_core::ForgeError::Io)?;

        Ok(serde_json::json!({
            "written": path,
            "bytes": content.len(),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== AionForge 2.0 · 完整交付演示 ===\n");

    // ---- 1. 组装核心栈（内存模式，零外部依赖）----
    let sdk = ForgeSdk::in_memory();
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let ws_root = std::env::temp_dir().join("forge-demo-ws");
    let workspace = Arc::new(WorkspaceManager::new(&ws_root)?);

    // ---- 2. 注册工具 ----
    let router = Arc::new(ToolRouter::new());
    router.register(Box::new(WriteFileTool::new()))?;
    use forge_exec::EchoTool; router.register(Box::new(EchoTool::new()))?;
    println!("[1] 工具注册完成: write_file (WorkspaceWrite)");

    // ---- 3. 创建任务（Command 验收——验证阶段独立于执行阶段运行）----
    let verify_cmd = "echo Hello AionForge"; // 跨平台一致（原 cfg! 分支两臂相同）
    let task = sdk
        .create_task(
            "输出 Hello AionForge".to_string(),
            vec![],
            vec![forge_task::AcceptanceCriterion {
                id: "AC-1".into(),
                description: "输出包含 Hello AionForge".into(),
                check: forge_task::CheckSpec::Command(verify_cmd.into()),
            }],
        )
        .await?;
    println!("[2] 任务已创建: {} ({:?})", task.goal, task.status);

    // ---- 4. 端到端编排 ----
    println!("[3] 编排器就绪（capability=write_file, timeout=30s）\n");

    let deps = OrchestratorDeps {
        router: router.clone(),
        policy: Arc::new(DemoAllowAll),
        verifier_cmd: Arc::new(CommandVerifier),
        verifier_file: Arc::new(FileVerifier),
        evidence: evidence.clone(),
        workspace: workspace.clone(),
        timeout: Duration::from_secs(30),
        // ORCH-003：演示默认配置——有界重试先行，未接 LLM 重规划器
        recovery: Arc::new(forge_recovery::BoundedRetryStrategy::default()),
        replanner: None,
        max_replans: 1,
        planner: None,
    };

    // 编排器 capability = "write_file"，即计划中的步骤会调用此工具
    let orch = Orchestrator { capability: "echo".into(), timeout: Duration::from_secs(30) };

    let report = sdk.run_end_to_end(&task.id, &deps, &orch).await?;

    // ---- 6. 打印报告 ----
    println!("[4] 编排完成！报告：");
    println!("  任务 ID:      {}", report.task_id);
    println!("  最终状态:     {:?}", report.final_status);
    println!("  门禁通过:     {}", report.gate.passed);
    println!("  执行波次:     {}", report.execution.waves);
    println!("  完成步骤数:   {}", report.execution.completed.len());
    println!("  验证条目数:   {}", report.verifications.len());
    for v in &report.verifications {
        println!("    [{:?}] {}: {}", v.verdict, v.criterion_id, v.reason);
    }
    println!("  证据数量:     {}", report.evidence_ids.len());

    if report.gate.passed {
        println!("\n✅ 交付通过全部门禁！");
    } else {
        println!("\n❌ 门禁拒绝。失败项: {:?}", report.gate.failed);
    }

    // ---- 7. 清理 ----
    std::env::set_current_dir("..")?;
    println!("\n=== 演示结束 ===");

    Ok(())
}
