# IMPROVE-9 Agent 生成 Plan → 传 Forge 执行（路径 B）施工规格

> **入库归档说明**：本文件为 `docs/` 归档副本；下发用原稿为工作区外层 `build_improve9.md`（内容与本文件一致，修订以原稿为准）。起草：Cline 2026-09-22，状态：待 P8 批准（零代码，未施工）。

> **文档编号**：AF-BP-IMP9-001
> **性质**：施工级规格（契约/校验链/降级路径冻结，低级模型可直接执行）
> **审批状态**：⏳ **待 P8 批准**（本草稿由 Cline 依项目所有人 2026-09-22 立项指示起草；批准前禁止动代码，仅本文档为交付物）
> **立项依据**：`roadmap_glm_queue.md` PACK-3 已拍板路径 B——由 Agent 侧模型产出 Plan，传给 Forge 执行（而非 Forge 内部自规划）；本规格补齐其"边界待规格化"两项：Plan 跨 agent 传输契约、Forge 侧接收入口与既有 orchestrate 链的关系。
> **实况基线**：`master` HEAD `003ba08`（行号按此基线实测；施工前若 HEAD 已移动，先重跑第 2 章自检重新定位）。
> **编号纪律**：任务 ID `IMPROVE-9`；分叉自 **D18** 起（D15/D16 被 KNOW 占用，D17 被 IMPROVE-10 占用）；风险自 **R7-025** 顺延（R7-024 已被 2026-09-22 本机基线实测的 Windows 平台 Command 验收缺口记录占用，见 WORKLOG R7-024）。
> **下发纪律**：执行方 ≠ 复核/签核方；台账经 `forge-worklog` CLI，禁手改三 MD。

---

## 第 0 章 执行须知（Agent 先读这段）

1. **本批只有一个真入口**：MCP 新增工具 `forge_plan_execute`（Agent 提交 Plan JSON → Forge 校验 → 执行）。**执行链零改动**：复用 `ForgeSdk::run_end_to_end` 全链路（计划→波次执行→验证→证据→门禁），通过既有 `OrchestratorDeps.planner` 注入槽位（`sdk/src/orchestrator.rs` L144-146）插入预制计划，不新增第二条执行链。
2. **只 use，不改**：`orchestrator.rs`、`forge-scheduler::run_plan`、`forge-dag::build_dag/topo_order`、`EngineStepExecutor`（含 `$sN.output` 引用解析）一律禁改。
3. **验收权不外包**：Plan 契约**不含验收字段**；验收永远来自 `task.acceptance`（空验收禁令、AllPass 门禁不动）。Agent 不能通过 Plan 弱化验收——这是 P7 的结构性防线，冻结。
4. **离线零回归**：`forge_orchestrate` 既有行为一字不变（含 LLM/验收驱动规划器装配分支）；本批工具独立注册、独立 allowlist 闸。
5. Windows 编译带 `-j 2` 与 `RUST_MIN_STACK=16777216`；台账提交号必须 git 对象库真实可 `git cat-file -e`。
6. 任何与本文冲突的实况，停下上报规划层，不得自行裁量扩界。

---

## 第 1 章 目标与范围

**目标**：外部 Agent（经 MCP stdio）提交自有 Plan JSON，Forge 先做**结构 + 能力白名单**双重校验，再走既有 CPEVR 闭环真实执行；步骤失败经既有 RecoveryStrategy 重试后仍败 → 升级（`escalated_to_human=true`）并把失败细节返回 Agent，**重规划责任在 Agent 侧**（改计划后再次提交，即路径 B 语义）。

**范围内（In）**：
- `planning/planner/src/prefab.rs`（新建）：`PrefabPlanner`——`Planner` trait 的预制计划适配器。
- `planning/dag/src/validate.rs`（新建）：`validate_plan_structure(&Plan)` 纯函数（复用 `build_dag` 环检测/悬空依赖检测 + 规模上限）。
- `capability/mcp/src/bin/forge_mcp_server/plan_execute_tools.rs`（新建）：`ForgePlanExecuteTool`（解析→接管字段→校验→装配→执行）。
- `orchestrate_tools.rs`：`construct_orchestrate_tool` 增 `"forge_plan_execute"` 注册分支（白名单点名制，与既有四工具同制）。
- `capability/mcp/tests/plan_execute.rs`（新建）：冻结测试矩阵（第 4 章）。

**范围外（Out，越界即退回）**：
- 不改 `sdk/src/orchestrator.rs` / `forge-scheduler` / `forge-dag` 既有函数 / `forge_orchestrate` 工具 / HTTP server 路由。
- 不加 HTTP 端点（Agent 接入面 = MCP，见 MCP-005；HTTP 侧留作后续独立立项）。
- 不改验收/Gate/Recovery 任何语义；不新增第三方 crate（仅 workspace 内 path 依赖，Cargo.lock 零新增外部拉取）。
- `PrefabPlanner` 不支持 `HumanApproval` 步骤（自动模式既有约束，orchestrator.rs L121-123），校验期直接拒绝。

---

## 第 2 章 现状证据（开工前自检必读）

代码事实（规划层 2026-09-22 在基线 `003ba08` 核到行级）：

| # | 事实 | 证据 |
|---|---|---|
| A1 | Plan 模型可序列化、含 id/task_id/steps/status | `planning/planner/src/model.rs` L20-58（`PlanStep` L22-31：`id/title/depends_on/action`；`StepAction` L33-45：`CallCapability{capability,input}` / `HumanApproval(String)`；`Plan` L47-58） |
| A2 | Planner trait 单方法、可注入 | `model.rs` L61-65 `async fn plan(&self, task:&Task)->ForgeResult<Plan>` |
| A3 | 编排器有规划器注入槽位 | `sdk/src/orchestrator.rs` L144-146 `deps.planner: Option<Arc<dyn Planner>>`；L179-182 `Some(p)→p.plan(&task)` / `None→SequentialPlanner` |
| A4 | HumanApproval 自动模式不支持 | `orchestrator.rs` L121-123 返回 `InvalidState("orchestrator auto-mode cannot handle HumanApproval")` |
| A5 | 跨步骤输出引用已就绪 | `orchestrator.rs` L86-90 `resolve_refs(input,&done)`（B-REAL-001C，`$sN.output[.path]`） |
| A6 | DAG 结构校验能力已在仓 | `planning/dag/src/graph.rs` L20-54 `build_dag(&Plan)`：自依赖→InvalidState(L28-33)、悬空依赖→DependencyMissing(L35-40)、环→InvalidState(L48-51)；`topo.rs` L11 `topo_order` 兜底环检测 L62-66 |
| A7 | 依赖方向无环 | `planning/dag/Cargo.toml` L9 `forge-dag → forge-planner`；反向无依赖 → 校验器放 dag crate 零新边 |
| A8 | MCP 编排入口与能力清单来源 | `orchestrate_tools.rs` L310 `ForgeOrchestrateTool`；L372-376 `exec_router_for(&workdir).list()` 得可用能力名（白名单校验的事实源）；L449 `run_end_to_end` 调用点；L457-467 `construct_orchestrate_tool` 注册分支模式 |
| A9 | PlanId 生成器已在仓 | `core/runtime/src/id.rs` L54 `PlanId: "plan", new_plan_id` |
| A10 | 工具白名单提示词先例 | `planning/llm/src/llm_planner.rs` L76-77 `tools: Vec<String>` 白名单写入系统提示词（防模型发明工具）——本批把同一约束落到**校验层**（比提示词更强） |

---

## 第 3 章 契约设计（原样抄入）

### 3.1 `PrefabPlanner`（`planning/planner/src/prefab.rs`，新建）

```rust
//! 预制计划适配器（IMPROVE-9 路径 B）：Agent 侧产出的 Plan 经校验后由此插入既有编排链。

pub struct PrefabPlanner { plan: Plan }

impl PrefabPlanner { pub fn new(plan: Plan) -> Self { Self { plan } } }

#[async_trait::async_trait]
impl Planner for PrefabPlanner {
    async fn plan(&self, _task: &forge_task::Task) -> forge_core::ForgeResult<Plan> {
        Ok(self.plan.clone()) // 已校验，原样返回；task 仅作签名兼容
    }
}
```

### 3.2 `validate_plan_structure`（`planning/dag/src/validate.rs`，新建）

```rust
pub const FORGE_PLAN_MAX_STEPS_DEFAULT: usize = 32;          // env FORGE_PLAN_MAX_STEPS 可调
pub const FORGE_PLAN_MAX_INPUT_BYTES_DEFAULT: usize = 65_536; // 单步 input JSON 上限, env 可调

/// 结构校验（纯函数，零 IO）。冻结检查序：
/// 1. steps 非空；2. step id 非空且唯一；3. steps 数 ≤ 上限；
/// 4. 逐步 input 序列化字节 ≤ 上限；5. build_dag（自依赖/悬空依赖/环，A6 复用）
pub fn validate_plan_structure(plan: &Plan) -> ForgeResult<()>
```
> 错误文案冻结：`"plan: empty steps"` / `"plan: empty step id"` / `"plan: duplicate step id <id>"` / `"plan: too many steps (<n> > <cap>)"` / `"plan: step <id> input too large"`；build_dag 错误原样透传。

### 3.3 MCP 工具 `forge_plan_execute`（`plan_execute_tools.rs`，新建）

**descriptor 描述冻结**：`"Execute an agent-supplied plan for a task: validate structure + capability whitelist, then run the existing plan -> wave execute -> verify acceptance -> gate chain. Returns OrchestratorReport JSON. Replanning is the agent's responsibility: on failure the report carries escalated_to_human + failure detail; submit a revised plan in a new call."`

**input schema 冻结**：
```json
{
  "type": "object",
  "properties": {
    "task_id": { "type": "string", "description": "Task ID to execute the plan against" },
    "plan": {
      "type": "object",
      "properties": {
        "steps": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "id": { "type": "string" },
              "title": { "type": "string" },
              "depends_on": { "type": "array", "items": { "type": "string" } },
              "action": { "oneOf": [
                { "type": "object", "properties": { "CallCapability": { "type": "object", "properties": { "capability": {"type":"string"}, "input": {} }, "required": ["capability","input"] } }, "required": ["CallCapability"] },
                { "type": "object", "properties": { "HumanApproval": { "type": "string" } }, "required": ["HumanApproval"] }
              ] }
            },
            "required": ["id", "title", "action"]
          }
        }
      },
      "required": ["steps"]
    },
    "workspace_task": { "type": "string", "description": "Optional prior task ID to reuse its workspace" }
  },
  "required": ["task_id", "plan"]
}
```
> `depends_on` 缺省 = `[]`。反序列化目标直接复用 `Vec<PlanStep>`（serde 既有，A1），不新造 DTO。

**服务端接管字段（冻结）**：`plan.id = new_plan_id()`（A9）、`plan.task_id = 入参 task_id`、`plan.status = PlanStatus::Ready`——客户端传这三个字段一律忽略（防伪造计划谱系）。

**校验链（顺序冻结，任一失败即返回错误，不进入执行）**：
1. `validate_plan_structure(&plan)`（3.2）
2. **HumanApproval 拒绝**：任一步 `HumanApproval` → `InvalidState("forge_plan_execute: HumanApproval not supported in auto mode (step <id>)")`（A4）
3. **能力白名单**：逐步 `capability ∈ exec_router_for(&workdir).list()`（A8 同一事实源）；缺失 → `InvalidState("forge_plan_execute: unknown capability '<name>' (step <id>); available: <逗号分隔清单>")`（错误透传可用能力，IMPROVE-6/7 自我修正血统）

**装配（冻结）**：`deps.planner = Some(Arc::new(PrefabPlanner::new(plan)))`；`replanner = None`（重规划责任在 Agent 侧）；**不接受** `max_replans` 入参（schema 无此字段）；其余（recovery/verifier/evidence/workspace）走 `make_deps_for_full` 既有路径。之后 `run_end_to_end` 一字不改。

### 3.4 失败语义（冻结）

步骤失败 → 既有 `RecoveryStrategy` 有界重试 → 仍败且 `replanner=None` → 既有升级路径（`escalated_to_human=true`，Session 留痕，orchestrator.rs L290-319）→ `OrchestratorReport` 原样返回 Agent（含 `execution.failed` 细节，IMPROVE-6 透传链）。Agent 改计划后**新调用**提交（每次调用 = 新 plan_id，计划谱系清晰）。

### 3.5 注册

`construct_orchestrate_tool` 增分支：`"forge_plan_execute" => Some(Box::new(ForgePlanExecuteTool::new(ctx.clone())))`——经 `FORGE_TOOLS_BUILTIN` 点名注册、`FORGE_MCP_ALLOWLIST` 调用闸，与既有四工具同制（能力先注册后使用）。

---

## 第 4 章 测试矩阵（新建 `capability/mcp/tests/plan_execute.rs`；测试名冻结；全程不触网）

| # | 测试 | 断言 |
|---|---|---|
| 1 | `plan_execute_happy_path_with_step_ref` | 两步计划（write_file → read_file，`$s1.output` 引用），`final_status==Completed`，报告 plan.id 为服务端生成（≠客户端伪造值） |
| 2 | `plan_execute_rejects_unknown_capability` | capability `nonexistent_tool` → InvalidState，文案含 `unknown capability` 且列出可用清单 |
| 3 | `plan_execute_rejects_cycle` | s1↔s2 互依 → InvalidState 含 `cycle` |
| 4 | `plan_execute_rejects_dangling_dependency` | depends_on 指向不存在步骤 → DependencyMissing |
| 5 | `plan_execute_rejects_duplicate_step_ids` | 重复 id → InvalidState 含 `duplicate step id` |
| 6 | `plan_execute_rejects_human_approval` | HumanApproval 步骤 → InvalidState 含 `HumanApproval not supported` |
| 7 | `plan_execute_rejects_oversized_plan` | 33 步（超默认 32 上限）→ InvalidState 含 `too many steps` |
| 8 | `plan_execute_failure_escalates_to_agent` | 必败步骤（工具执行失败）→ `final_status==Failed` 且 `escalated_to_human==true` 且 `replans_used==0`（无 replanner） |
| 9 | `plan_execute_gate_governs_completion` | 步骤全成功但 task 验收（FileContains）不满足 → `final_status==Failed`（验收权不外包，P7 防线实证） |
| 10 | `prefab_planner_returns_plan_clone`（planner crate 单测） | PrefabPlanner.plan() 原样返回注入计划 |
| 11 | `validate_plan_structure_accepts_linear_and_diamond`（dag crate 单测） | 线性 + 菱形依赖均通过 |

> 异步一律 `tokio::time::timeout` 护栏（R1-094 先例）；测试全程离线（内存栈 + 内置工具）。

---

## 第 5 章 门禁 G-IMP9

```powershell
$env:RUST_MIN_STACK = '16777216'
cargo clippy --workspace --all-targets --features forge-mcp/server-bin -j 2 -- -D warnings
cargo test     --workspace --features forge-mcp/server-bin --no-fail-fast -j 2
cargo test -p forge-mcp --test plan_execute -j 2
```

| # | 门禁 | 判据 |
|---|---|---|
| G1 | clippy 零告警 | `--features forge-mcp/server-bin -D warnings` 通过 |
| G2 | 数量下限 | 全量 test 通过数 ≥ 施工前实测基线 + 本批新增（≥11）；0 failed |
| G3 | 离线零回归 | `forge_orchestrate` 既有用例全绿；无 LLM/PG 环境行为一字不变 |
| G4 | 验收防线实证 | 测试 #9 绿：外部计划无法绕过 task 验收 |
| G5 | 台账 | `progress.json` 建 `IMPROVE-9`（真实 commit）；worklog R1 + D18 决议落地记录；`forge-worklog export` 重生成三 MD（禁手改） |

---

## 第 6 章 D18 决议与并发安全

**D18（接收入口形态）— 建议采纳 B：新独立工具 `forge_plan_execute`（入口并列，执行链复用）**。

| 方案 | 做法 | 取舍 |
|---|---|---|
| A | `forge_orchestrate` 增可选 `plan` 字段，有 plan 走预制、无 plan 走自规划 | 工具数不增，但单工具双模态：descriptor 语义模糊、既有用例回归面扩大、allowlist 无法对两种模式分别闸控 |
| **B（建议）** | 新工具 `forge_plan_execute`，`forge_orchestrate` 一字不动 | 既有工具零回归；allowlist 独立闸；工具语义单一（"能力先注册后使用"纪律对齐）；代价 = 多一个 descriptor |

- 待项目所有人拍板；若裁 A，本规格 3.3 节装配逻辑不变，仅落点改为 `forge_orchestrate` 的可选字段分支。
- **并发安全**：本批文件面 = `planning/planner/src/prefab.rs`、`planning/dag/src/validate.rs`、`capability/mcp/{src/bin/forge_mcp_server/plan_execute_tools.rs, tests/plan_execute.rs}` + `orchestrate_tools.rs` 一行注册分支。与在途批次零交集；共享仅台账 JSON，经 `forge-worklog` CLI 持锁串行化（COMP-003b）。

---

## 第 7 章 交付物与报告规则

执行方交付须含：① 改动文件清单 + `ForgePlanExecuteTool::invoke` 校验链关键 diff；② `plan_execute.rs` 全绿原始输出；③ 全量门禁尾段（含基线换算，注明 G3 离线不退）；④ Cargo.lock 无新增外部拉取证明；⑤ 台账五件套（G5）。全部命令本人实跑禁转述；执行方 ≠ 复核方。

---

## 风险增量

| 编号 | 风险 | 缓解 |
|---|---|---|
| R7-025 | Agent 提交超大/畸形 Plan 消耗资源 | 步骤数 + 单步 input 字节双上限（3.2），校验期拒绝不进入执行 |
| R7-026 | Agent 发明不存在的工具名 | 能力白名单校验 + 错误文案回传可用清单（3.3 校验 3） |
| R7-027 | 外部计划绕过验收完成条件 | Plan 契约无验收字段 + 测试 #9 实证 Gate 仍裁决（第 0 章纪律 3） |
| R7-028 | 预制计划与 replanner 混用导致版本谱系混乱 | `replanner=None` 冻结，重规划只能 Agent 外部发起新 plan_id（3.4） |