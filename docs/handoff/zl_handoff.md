# zl 工具组 交接说明（给接手 AI）

## 一、要做什么
把 9 个"感知/策略层"纯计算工具移植进新 forge，落到 `D:\test\aionui\新forge\aion-forge\tools\zl\src\tool.rs`，并接进 `build_router`，把工具总数从 25 推到 34。

## 二、9 个工具清单（纯计算、零外部依赖、权限全 ReadOnly）
| # | 工具名 | 输入 | 输出 |
|---|--------|------|------|
| 1 | check_sufficiency | task, context | 是否充足 + 缺失项 + 分数 |
| 2 | verify_result | criteria, result | 是否通过 + 违规项 |
| 3 | compile_contract | spec | 目标/约束/验收/风险（结构化合同） |
| 4 | detect_drift | intent, actual | 漂移等级 + 偏移点 |
| 5 | contradiction_analyze | a, b | 是否矛盾 + 理由 |
| 6 | prompt_audit | prompt | 问题列表 + 质量分 |
| 7 | verify_contract | contract, result | 是否满足合同 |
| 8 | （补满到9）strategic_plan | task | 攻防三阶段计划 |
| 9 | （补满到9）evolver_governance | task | 风险分级 + 推荐能力 |

参考旧 forge 源码：`D:\test\aionui\forge\aion-router\src\builtins\zl.rs` 和 `prompt_audit.rs`（旧 forge 有 check_sufficiency/verify_result/compile_contract/detect_drift/contradiction_analyze 5 个真实实现，可直接移植；prompt_audit 旧 forge 也有，可移植或重写）。

## 三、必须照抄的真实 API 模式（这是之前反复挂的根因）
新 forge 的 Tool 写法，**严格仿照 `tools/parsing/src/tool.rs`**，别凭记忆写：

```rust
use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::json;

fn err(msg: impl Into<String>) -> ForgeError {
    ForgeError::InvalidState(msg.into())
}

pub struct XxxTool {
    descriptor: ToolDescriptor,
}
impl XxxTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "工具名".into(),
                description: "中文描述".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": { ... },
                    "required": [...]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}
#[async_trait]
impl Tool for XxxTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input.get("text").and_then(|v| v.as_str()).ok_or_else(|| err("text 必填"))?;
        Ok(json!({ "ok": true, ... }))
    }
}
```

关键点：
- `Tool` trait 来自 `forge_exec`（即 `execution/runtime` crate）
- `ForgeError`/`ForgeResult` 来自 `forge_core`（即 `core/runtime`）
- 每个工具是"单字段 `descriptor` struct + new() 预构造"模式
- 每个 crate 末尾要有 `pub fn register_all(router: &ToolRouter)`，逐个 `router.register(Box::new(XxxTool::new()))`
- 测试用 `#[tokio::test]` async

先读 `tools/parsing/src/tool.rs` 和 `tools/parsing/src/lib.rs` 照抄结构（lib.rs 就两行：`pub mod tool; pub use tool::*;`）。

## 四、已存在的文件（别重建）
- `tools/zl/Cargo.toml` —— 已建好，依赖对（forge-core、forge-exec、anyhow、serde、serde_json、tokio、async-trait、tempfile）
- `tools/zl/src/lib.rs` —— 已建好（两行导出）
- 缺的就是 `tools/zl/src/tool.rs`（核心代码）

## 五、还要接的三处（zl 写完后）
1. **根 `Cargo.toml` 的 workspace members** 加 `"tools/zl"`（在 `"tools/metatool"` 那行附近）
2. **`cli/Cargo.toml`** 加 `forge-tools-zl = { path = "../tools/zl" }`
3. **`cli/src/mcp_server.rs`**：
   - 顶部 `use forge_tools_zl as zl;`
   - `use forge_tools_zl::register_all as register_zl;`
   - `build_router()` 里加 `register_zl(&router).expect("zl tools register");`
   - 两处测试断言 `assert_eq!(all.len(), 25 ...)` 和 `assert_eq!(arr.len(), 25)` 改成 34（25 + 9）

## 六、验证（必须跑，绿了才算完）
```
Set-Location "D:\test\aionui\新forge\aion-forge"
rtk proxy cargo test -p forge-tools-zl
rtk proxy cargo test -p forge-cli
rtk proxy cargo test --workspace
```
全绿（0 失败、exit 0）才算交付完成。

## 七、为什么之前的 AI 没完成
反复写 `tools/zl/src/tool.rs` 时工具调用漏了必填的 content 参数 / 工具名写错，导致文件一直空着；且没先核对 parsing 里的真实 API 就凭记忆写。接手后**先读 parsing/tool.rs 再写**，别臆造符号。

## 八、硬约束（与 ready_to_kickoff.json 一致）
- 不碰 `cli/src/main.rs`
- fs、search、parsing、text、metatool 只增不改
- zl 是纯计算，权限全 ReadOnly，不引入网络/外部依赖
- 生成物过 review-gate 再合入（别静默写）
