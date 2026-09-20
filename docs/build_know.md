# Aion Forge KNOW 批施工包：让默认单机 serve 的知识不再重启即丢

> **文档编号**：AF-BP-KNOW-001
> **性质**：施工级规格（接口契约冻结、测试矩阵、DoD 命令齐备，低级模型可直接执行）
> **审批状态**：✅ **P8 已批准**（批准人：项目所有人；批示 2026-09-20，口径"批准 P8，G6 收口后下发"；G6 已 @`a2c785c` 关账）。批准范围 = KNOW-001A + KNOW-001B 全包；第 6 章 **D15 按默认建议采纳**（缺省单机 serve 翻转为文件持久，逃生阀 `FORGE_KNOWLEDGE_PERSIST=0`），**D16 维持 Out of scope**。此后范围变更仍走 R6 决议，执行方不得自行扩界。
> **下发纪律**：执行方（写代码）与复核/签核方不得为同一角色；本批以 `know_001a_kickoff.json` 为施工单。
> **基线漂移说明**：G6 收口提交 `a2c785c` 仅改台账/证据（`progress.json`/`worklog.json`/`handoff.json` + 三份 md 视图 + artifacts），**未触碰任何 `.rs`**，故第 2 章 `server/src/lib.rs` 行号相对 `7874673` 零漂移，可直接照抄。
> **立项依据**：用户 2026-09-19 批示——B-REAL 批（AF-BP-BREAL-001）代码断链已闭合（唯 G6 真模型签核暂挂，另见 `g6_breal_signoff_runbook.md`），下一步处理 handoff `advice` 遗留三项中范围最收敛的 **R7-015 knowledge 持久化断链**。它与 B-REAL 同性质：**能力已搬来，但默认运行时尚未真正用上**。
> **实况基线**：`master` HEAD `7874673`（第 2 章行号按此基线实测；施工前若 HEAD 已移动，先重跑第 2 章自检命令重新定位）。
> **编号纪律**：风险自 **R-49** 顺延（R-41~R-48 已被 B-REAL 占用）；分叉自 **D15** 起（D13/D14 已被 B-REAL 占用）。
> **口径统一声明**：本批任务 ID 前缀 `KNOW-*`。R7-015 已声称"断链修复"（卡 Completed / commit `15910c1`），但只修了 **PG 分支 + CLI**，**缺省非 PG 的 serve 运行时仍漏**——本批即补这最后一处，不复盘已修部分。

---

## 第 0 章 执行须知（Agent 先读这段）

1. **本批只有一个真问题**：`forge serve` 在**未设 `FORGE_PG_URL`** 时（绝大多数单机/开发场景），运行时 `AppState.knowledge` 是 `InMemoryKnowledgeBase` → 编排失败知识（ingest）与 `/knowledge/failures`（recall）都活在内存里，**进程一重启全丢**。
2. **改动面极小且被刻意限定在 `run_from_env()` 的 `Err(_) =>` 一个分支**。**严禁**改 `AppState::in_memory()` 构造器本身——它有 **29 处调用点（绝大多数是测试）**，改它会 ① 打挂"内存态"测试前提，② 让每次测试跑都往真实 `~/.aion-forge/knowledge.jsonl` 写盘、互相污染。
3. 遵守项目既有纪律：不新增依赖、不动 trait、不动工具 output 字段、不改策略/沙箱链；Windows 编译带 `-j 2` 与 `RUST_MIN_STACK`；台账提交号必须是 git 对象库里真实存在的 hash。
4. 任何与本文冲突的实况，**停下上报规划层**，不得自行裁量扩界。

---

## 第 1 章 目标与范围

**目标**：让默认单机 `serve`（无 PG）路径的 `knowledge` 与 PG 分支、CLI 三者行为一致——**文件持久、跨重启不丢**，且对现有测试零扰动。

**范围内（In）**：
- `server/src/lib.rs` `run_from_env()` 非 PG 分支的 `knowledge` 装配（env 门控，缺省持久）。
- 一条端到端"重启不丢"证据测试（同进程内以两个共享同一文件路径的实例模拟跨重启，避免真起停服务）。

**范围外（Out，越界即退回）**：
- 不改 `InMemoryKnowledgeBase` / `FileKnowledgeBase` 任何实现与 `FailureKnowledgeBase` trait。
- 不改 `AppState::in_memory()` / `AppState::new()` 构造器（保护 29 调用点）。
- 不改 ingest（L549）/ recall（L915）业务逻辑。
- 不引入 PG、不引入新 crate 依赖（`FileKnowledgeBase`、`knowledge_file()` 已在 server 可见）。

---

## 第 2 章 现状断链证据（开工前自检必读）

以下均为**代码事实**（规划层 2026-09-19 在基线 `7874673` 逐一核到行级）：

| # | 断链 | 证据 |
|---|---|---|
| K1 | serve 非 PG 分支仍用内存知识库 → 单机重启即丢 | `server/src/lib.rs` L1410 `Err(_) => { … AppState::in_memory() }`，而 `in_memory()` L147 `knowledge: Arc::new(InMemoryKnowledgeBase::default())` |
| K2 | 同一条 `serve`，**PG 分支却已持久** → 行为随"是否配 PG"悄然分裂 | L1400 `knowledge: Arc::new(FileKnowledgeBase::new(knowledge_file()))` |
| K3 | CLI 侧已持久（R7-015 已修），佐证"缺省 serve 分支是漏改而非有意" | `cli/src/main.rs` L123 `let kb = FileKnowledgeBase::new(knowledge_file())` |
| K4 | ingest / recall 均跑在 `st.knowledge` 上，故 K1 直接决定二者是否可跨重启 | ingest：L549（编排失败写 `KnowledgeEntry`）；recall：L915 `GET /knowledge/failures` → `st.knowledge.search(...)` |
| K5 | `knowledge` 字段是 trait 对象，File/InMemory 同实现该 trait → 替换是 drop-in，无签名波及 | 字段 `pub knowledge: Arc<dyn FailureKnowledgeBase>`；trait `ingest/search/all`（`failures.rs` L51-L59）；两实现 `failures.rs` L77 / `failures_file.rs` L68 |
| K6 | 路径已支持 env 覆盖，测试隔离有现成抓手 | `knowledge_file()`：`$FORGE_KNOWLEDGE_FILE` 否则 `~/.aion-forge/knowledge.jsonl`（`failures_file.rs` L11-L21） |

**自检命令**（施工前跑，确认行号未漂移）：
```powershell
cd d:\test\aionui\新forge\aion-forge
Select-String -Path server\src\lib.rs -Pattern 'AppState::in_memory\(\)|InMemoryKnowledgeBase::default\(\)|FileKnowledgeBase::new' | ForEach-Object { $_.LineNumber.ToString()+': '+$_.Line.Trim() }
(Select-String -Path (Get-ChildItem -Recurse -Filter *.rs | Where-Object { $_.FullName -notmatch '\\target\\' }).FullName -Pattern 'AppState::in_memory\(\)' | Measure-Object).Count   # 期望 29（含定义外调用）
```

---

## 第 3 章 任务包

### 3.1 KNOW-001A 装配面：非 PG serve 分支切文件知识库（env 门控，缺省持久）

**文件**：`server/src/lib.rs`（仅 `run_from_env()` 的 `Err(_) =>` 分支，约 L1410）。

**接口契约（冻结）**：
- 新增 env 门控 `FORGE_KNOWLEDGE_PERSIST`：值 `"0"` ⇒ 保持旧内存行为（逃生阀）；**未设或非 "0" ⇒ 缺省持久**（本批就是要翻转缺省）。
- 持久实例路径**必须**经 `knowledge_file()` 取得（继承 `FORGE_KNOWLEDGE_FILE` 覆盖能力），**禁止硬编码** `~/.aion-forge`。

**改造规则**：
- **R1** 只替换 `Err(_) =>` 分支返回的 `AppState` 的 `knowledge` 字段；PG 分支（L1400）**不动**（已正确）。示例（语义等价即可，命名可调）：
  ```rust
  Err(_) => {
      println!("storage: in-memory");
      let mut state = AppState::in_memory();
      // KNOW-001A: 缺省单机 serve 也持久化知识（对齐 PG 分支 L1400 与 CLI main.rs:123）。
      // 不改 in_memory() 构造器本体（29 测试调用点依赖内存态）。
      if std::env::var("FORGE_KNOWLEDGE_PERSIST").ok().as_deref() != Some("0") {
          state.knowledge = Arc::new(FileKnowledgeBase::new(knowledge_file()));
          eprintln!("knowledge: file-backed ({})", knowledge_file().display());
      }
      state
  }
  ```
- **R2** `FileKnowledgeBase` / `knowledge_file` 若尚未在 server `use`，**复用 PG 分支已有的导入**（L1400 已在用，说明符号在作用域内），不新增重复 `use`，否则 clippy `-D warnings` 挂 `unused_imports`。
- **R3** **不得**改 `AppState::in_memory()`（L131-L155）与 `AppState::new()`（L156-L177）里的 `InMemoryKnowledgeBase::default()`。
- **R4** 权限面零改动；本批与工具/沙箱/策略链无关，勿顺手动 `select_command_verifier` 等（沿用 B-REAL R5 精神）。

**测试矩阵**（新建 `server/tests/know.rs`；测试名冻结；异步用例带 `tokio::time::timeout` 护栏——R1-094 教训）：

| # | 测试 | 断言 |
|---|---|---|
| 1 | `in_memory_ctor_still_uses_memory_kb`（零扰动护栏） | `AppState::in_memory()` 仍为内存态：连续两次取 `all()` 互不影响、进程内可见即可（不校验落盘）。**这条锁死 R3**：证明装配修复没串改构造器 |
| 2 | `persist_env_gate_reads_flag` | 设 `FORGE_KNOWLEDGE_PERSIST=0` 与非 "0" 两态，验证门控判定纯函数（建议将判定抽成 `fn knowledge_persist_enabled() -> bool` 以便测；无该辅助函数则此条降为代码走查项并在报告注明） |
| 3 | `file_kb_path_honors_env_override` | 设 `FORGE_KNOWLEDGE_FILE=<temp>/kb.jsonl`，`knowledge_file()` 返回该路径（证明测试可隔离、不污染真实 home；对应 K6） |

> 说明：KNOW-001A 的"装配真的走了 File"由 KNOW-001B 的持久化 e2e 行为级证明，避免只对布尔返回值断言的假修复（AF-AUDIT-003 N2 教训）。

**DoD**：
```powershell
cargo test -p forge-server --test know -j 2
cargo test -p forge-server --test orch101 --test v8 --no-fail-fast -j 2   # 既有冻结测试零回归
cargo clippy --workspace --all-targets -j 2 -- -D warnings               # 零告警硬闸
```
**提交**：`KNOW-001A: 缺省单机 serve 知识库改文件持久(env 门控, 缺省持久, 测试零扰动)`

---

### 3.2 KNOW-001B 数据面：重启不丢的持久化端到端证据

**目标**：用行为证据证明"ingest 落盘 → 新实例 search 命中"，即 K1 真被闭合。

**文件**：并入 `server/tests/know.rs`。

**测试矩阵**（测试名冻结；全程用 `FORGE_KNOWLEDGE_FILE` 指向 `tempfile` 路径，禁写真 home）：

| # | 测试 | 断言 |
|---|---|---|
| 4 | `knowledge_survives_instance_restart` | 同一 `path` 建 `FileKnowledgeBase` 实例 A → `ingest(KnowledgeEntry{ record: <失败1>, tool: Some("orchestrate"), related_evidence: [] })`；drop A；再建实例 B（同 path）→ `search(None, Some("orchestrate"), None)` 命中该条，`all()` 非空。**跨实例即等价跨重启**（K5 证明二者读同一 JSONL） |
| 5 | `empty_file_yields_no_entries` | 指向不存在/空文件的实例 → `search/all` 返回空、不 panic（畸形/空文件鲁棒性，呼应 `failures_file.rs` L197 malformed 单测口径） |

**DoD**：
```powershell
cargo test -p forge-server --test know -j 2          # #1-#5 全绿
cargo clippy --workspace --all-targets -j 2 -- -D warnings
```
**提交**：`KNOW-001B: 知识持久化重启不丢 e2e(两实例共享文件路径, tempdir 隔离)`

---

## 第 4 章 门禁 G-KNOW

```powershell
$env:RUST_MIN_STACK = '16777216'
cargo check  --workspace -j 2
cargo clippy --workspace --all-targets -j 2 -- -D warnings     # 零告警硬闸
cargo test   --workspace --no-fail-fast -j 2
```

| # | 门禁 | 判据 |
|---|---|---|
| G1 | 三命令全绿 | 实跑贴尾段（含各 `test result:` 行）。**数量下限**：B-REAL 收口基线 `7874673` 全量实跑为 **544 passed / 0 failed / clippy 0 warn**；交付通过数须 ≥ 544 + 本批新增用例数（≥5），少于即"有用例消失"退回 |
| G2 | 零扰动 | `orch101`（含 #13）与 `v8` 全绿；`know.rs` #1 证 `in_memory()` 未被串改 |
| G3 | 断链闭合 | `knowledge_survives_instance_restart` 通过（行为级，不接受仅装配布尔断言） |
| G4 | 测试不污染 home | 全程经 `FORGE_KNOWLEDGE_FILE`/tempdir；施工后 `%USERPROFILE%\.aion-forge\knowledge.jsonl` 不因跑测试被新建/追加（执行前后各查一次并附结果） |
| G5 | 台账 | `progress.json` 建 `KNOW-001A/001B`（真实 commit）；`WORKLOG` 加 R1 条目；因涉 R7-015 收口，须加一条 worklog 注明"R7-015 缺省 serve 分支持久化补全（本批）"；`HANDOFF.md`/`PROGRESS.md` 经 `forge-worklog export` 重生成（禁手改 md） |

---

## 第 5 章 交付物与报告规则

执行方交付须含：
1. 改动文件清单 + `run_from_env()` 分支前后 diff 片段；
2. `know.rs` #1-#5 全绿原始输出（含 `test result:` 行）；
3. G1 全量三命令原始尾段（通过数 ≥ 下限）；
4. G4 的 home 文件前后对比证据；
5. 台账落档证明：`progress.json` diff + worklog 新条目编号 + export 重生成时间戳。

**报告完整性规则**：必须本人实跑，不得转述他人"已绿"；执行方与签核/复核方不得为同一角色。开工前若 HEAD 已越过 `7874673`，第 2 章行号与 G1 的 544 下限须按自检命令重测并在报告注明新基线。

---

## 第 6 章 风险与分叉

| # | 风险 / 分叉 | 处置 |
|---|---|---|
| **R-49** | 误改 `AppState::in_memory()` 会打挂 29 处测试并让 CI 污染真实 home | 范围硬锁 `run_from_env()` `Err` 分支；`know.rs` #1 行为护栏；G4 探针 |
| **R-50** | 缺省翻转为"持久"后，任何真起 `serve` 的集成/GA 脚本若无 PG，会开始写 `~/.aion-forge/knowledge.jsonl` | 经核 `ga_acceptance.ps1` 起服务时设 `FORGE_PG_URL`（走 PG 分支，不受本改影响）；仍建议 harness 显式设 `FORGE_KNOWLEDGE_FILE` 到临时路径；逃生阀 `FORGE_KNOWLEDGE_PERSIST=0` |
| **R-51** | 文档/注释与代码漂移（B-REAL 已实证此类） | 交付物 #5 要求"改动分支内注释与本规格口径一致"，复核蓝军抽验一条 |
| **D15** | 缺省 serve 到底是"内存快"还是"文件持久"？ | **建议默认持久（本批采纳）**：知识丢失是缺陷不是性能诉求；内存态经 `FORGE_KNOWLEDGE_PERSIST=0` 仍可取，兼顾测试与极致启动场景。若项目所有人改判"缺省内存、按需持久"，则翻转门控默认值，本包其余不变 |
| **D16** | 是否顺带把 recall（`/knowledge/failures`）接进 planner 提示，形成"失败记忆喂规划"闭环？ | **本批不做，Out of scope**：那属规划面增强，应另立规格；本批只闭合持久化断链，避免扩界 |
