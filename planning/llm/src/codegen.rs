//! 单文件代码生成规划器（V5.1 CGN-001，AF-BP-V51-001 契约冻结）。
//!
//! "写软件"路径核心，对小上限模型鲁棒：两次**纯文本**调用，全程无 JSON——
//! 1. 问文件名（短输出，天然不受长补全截断影响）；
//! 2. 要完整文件内容（原始代码，strip 围栏后落盘）。
//!
//! 执行写入（write_file 工具）与运行验收由确定性引擎/Verifier 完成，LLM 只负责产出代码。
//!
//! 规则（build_v51.md）：
//! - R1 输出计划必须过 [`validate_plan`] 校验（与 [`LlmPlanner`] 同一门禁）；
//! - R2 `path` 禁止绝对路径与 `..`（与 WRT-001 双保险，规划层先拦一层）；
//! - R3 提示词模板置于 crate 内 const，禁止从 env/网络加载。

use crate::llm_planner::LlmPlanBackend;
use crate::validator::validate_plan;
use async_trait::async_trait;
use forge_api::ChatMessage;
use forge_core::{ForgeError, ForgeResult};
use forge_planner::{Plan, Planner, SequentialPlanner};
use forge_task::Task;
use std::path::Path;
use std::sync::Arc;

/// 系统提示词：文件名短输出（R3：crate 内 const）。
const NAME_SYS: &str = "Answer with a single output filename including extension (like hello.py or app.js). No explanation.";

/// 系统提示词：完整文件内容（R3：crate 内 const）。
const CODE_SYS: &str = "You are a senior engineer. Output ONLY the complete final content of the requested file. No markdown fences, no explanations.";

/// 单文件代码生成规划器。
pub struct SingleFileCodegenPlanner<B: LlmPlanBackend + ?Sized> {
    pub backend: Arc<B>,
    pub model: String,
    /// LLM 失败/输出不合规时的回退规划器（现状行为：顺序计划）。
    pub fallback: SequentialPlanner,
}

impl<B: LlmPlanBackend + ?Sized> SingleFileCodegenPlanner<B> {
    pub fn new(backend: Arc<B>, model: impl Into<String>) -> Self {
        Self {
            backend,
            model: model.into(),
            fallback: SequentialPlanner { capability: "echo".into() },
        }
    }
}

/// 去掉 markdown 代码围栏（模型常附 ``` 围栏，落盘需剥离）。
fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let mut lines: Vec<&str> = t.lines().collect();
    if lines.len() >= 2 {
        lines.remove(0); // ```lang
        if lines.last().map(|l| l.trim() == "```").unwrap_or(false) {
            lines.pop();
        }
        lines.join("\n")
    } else {
        t.to_string()
    }
}

/// 取第一个非空行作为文件名。
fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("output.txt")
        .to_string()
}

/// R2：规划层路径防线（与 WRT-001 防逃逸双保险）——禁止绝对路径与 `..`。
fn path_is_safe(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains("..")
        && !rel.starts_with('/')
        && !rel.starts_with('\\')
        && !rel.contains(':')
        && !Path::new(rel).is_absolute()
}

#[async_trait]
impl<B: LlmPlanBackend + ?Sized + 'static> Planner for SingleFileCodegenPlanner<B> {
    async fn plan(&self, task: &Task) -> ForgeResult<Plan> {
        // 失败即回退顺序计划（冻结测试 codegen_plan_fallback_on_llm_error）
        let run = async {
            // ① 文件名（短输出）
            let name_user = format!("Task: {}\nPick the best output filename.", task.goal);
            let name_raw = self
                .backend
                .complete(&self.model, &[ChatMessage::system(NAME_SYS), ChatMessage::user(name_user)])
                .await?;
            let filename = first_line(&strip_code_fence(&name_raw));

            // R2：规划层路径防线
            if !path_is_safe(&filename) {
                return Err(ForgeError::InvalidState(format!(
                    "codegen: unsafe path '{filename}'"
                )));
            }

            // ② 完整文件内容（纯文本，无 JSON 转义/截断问题）
            let mut ac_text = String::new();
            for a in &task.acceptance {
                ac_text.push_str(&format!("- {}: {}\n", a.id, a.description));
            }
            let code_user = format!(
                "Filename: {filename}\nIt will be verified by:\n{ac_text}\nGoal: {goal}\nWrite the complete file now.",
                goal = task.goal,
            );
            let raw = self
                .backend
                .complete(&self.model, &[ChatMessage::system(CODE_SYS), ChatMessage::user(code_user)])
                .await?;
            let content = strip_code_fence(&raw);

            Ok((filename, content))
        };

        match run.await {
            Ok((filename, content)) => {
                // R1：构造计划 JSON 后过 validate_plan 同一门禁
                let raw_plan = serde_json::json!({
                    "steps": [{
                        "id": "codegen",
                        "title": format!("生成 {filename}"),
                        "depends_on": [],
                        "action": {
                            "type": "call",
                            "capability": "write_file",
                            "input": { "path": filename, "content": content },
                        },
                    }],
                });
                validate_plan(&raw_plan, &task.id)
            }
            Err(_) => self.fallback.plan(task).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::TaskId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 顺序回放预设响应的离线 mock（与 llm_planner 测试同口径）。
    struct MockBackend {
        responses: Vec<String>,
        calls: AtomicUsize,
    }

    impl MockBackend {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses.into_iter().map(String::from).collect(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LlmPlanBackend for MockBackend {
        async fn complete(&self, _model: &str, _messages: &[ChatMessage]) -> ForgeResult<String> {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses.get(i).cloned().ok_or_else(|| {
                ForgeError::InvalidState("mock: no more canned responses".into())
            })
        }
    }

    fn demo_task() -> Task {
        Task::new(TaskId::new_task_id(), "write a hello file".into(), vec![], vec![])
    }

    /// 冻结测试（V5.1 CGN-001）：mock 返回合法输出 → 计划恰一步且 tool=write_file。
    #[tokio::test]
    async fn codegen_plan_single_step_write_file() {
        let planner = SingleFileCodegenPlanner::new(
            Arc::new(MockBackend::new(vec!["hello.py", "print('hello')"])),
            "mock-model",
        );
        let plan = planner.plan(&demo_task()).await.unwrap();
        assert_eq!(plan.steps.len(), 1, "计划必须恰一步");
        match &plan.steps[0].action {
            forge_planner::StepAction::CallCapability { capability, input } => {
                assert_eq!(capability, "write_file");
                assert_eq!(input["path"], "hello.py");
                assert_eq!(input["content"], "print('hello')");
            }
            other => panic!("应为 CallCapability 步骤: {other:?}"),
        }
    }

    /// 冻结测试（V5.1 CGN-001）：mock 报错 → 回退计划非空且不含 write_file。
    #[tokio::test]
    async fn codegen_plan_fallback_on_llm_error() {
        let planner = SingleFileCodegenPlanner::new(
            Arc::new(MockBackend::new(vec![])), // 无 canned 响应 → complete 报错
            "mock-model",
        );
        let plan = planner.plan(&demo_task()).await.unwrap();
        assert!(!plan.steps.is_empty(), "回退计划必须非空");
        let has_write_file = plan.steps.iter().any(|s| matches!(
            &s.action,
            forge_planner::StepAction::CallCapability { capability, .. } if capability == "write_file"
        ));
        assert!(!has_write_file, "回退计划不得含 write_file 步骤");
    }

    /// 冻结测试（V5.1 CGN-001）：mock 返回 `../x.rs` → 校验拒绝并回退。
    #[tokio::test]
    async fn codegen_plan_rejects_unsafe_path() {
        let planner = SingleFileCodegenPlanner::new(
            Arc::new(MockBackend::new(vec!["../x.rs", "evil()"])),
            "mock-model",
        );
        let plan = planner.plan(&demo_task()).await.unwrap();
        let has_write_file = plan.steps.iter().any(|s| matches!(
            &s.action,
            forge_planner::StepAction::CallCapability { capability, .. } if capability == "write_file"
        ));
        assert!(!has_write_file, "不安全路径必须被拒并回退到顺序计划");
    }

    #[test]
    fn strip_fence_variants() {
        assert_eq!(strip_code_fence("```\nabc\n```"), "abc");
        assert_eq!(strip_code_fence("```rust\nfn f(){}\n```"), "fn f(){}");
        assert_eq!(strip_code_fence("plain"), "plain");
    }

    #[test]
    fn path_safety_matrix() {
        assert!(path_is_safe("src/main.rs"));
        assert!(!path_is_safe("../x.rs"));
        assert!(!path_is_safe("/abs.rs"));
        assert!(!path_is_safe("C:\\x.rs"));
        assert!(!path_is_safe(""));
    }
}
