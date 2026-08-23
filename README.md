# Aion Forge 2.0

> AI 交付流水线核心 —— 以"验证即完成条件"为纪律的 Rust 实现版。

[![tests](https://img.shields.io/badge/tests-240%20passing-brightgreen)]() [![clippy](https://img.shields.io/badge/clippy-all--targets%20clean-blue)]()

## 这是什么

Aion Forge 2.0 是一个 Cargo workspace，实现九层架构中的核心层：
**Runtime / Agent / Task / Planning / Execution / Verification / Recovery / Capability / Product**，
并提供 HTTP 服务与真实模型接入。

- 规划文档回答"造什么、为什么"
- 施工包（build_a/b/c.md）回答"每一步写什么代码、凭什么算完成"

## 快速开始

```bash
cargo build --workspace          # 编译
cargo test --workspace           # 单元+集成测试（默认离线）
cargo clippy --workspace --all-targets   # 静态检查
cargo run -p forge-cli           # CLI: 输出版本
```

### 启动 HTTP 服务

```bash
cargo run -p forge-server              # 内存存储, 默认 :8080
FORGE_PORT=9000 \
FORGE_PG_URL=postgres://postgres:forge@localhost:15432/forge \
cargo run -p forge-server              # PostgreSQL 持久化
curl localhost:8080/health
```

或经 CLI：`cargo run -p forge-cli -- serve`

### 带基础设施的完整测试（可选）

| 环境变量 | 用途 | 本地容器 |
|---|---|---|
| `FORGE_PG_URL` | PG 持久化集成测试 | `podman run -d --name forge-pg -e POSTGRES_PASSWORD=forge -e POSTGRES_DB=forge -p 15432:5432 postgres:16-alpine` |
| `FORGE_MINIO_URL/AK/SK` | MinIO 对象存储集成测试 | `podman run -d --name forge-minio -p 19000:9000 minio/minio server /data` |
| `FORGE_LLM_BASE_URL/KEY` + `FORGE_LLM_LIVE=1` | 真实模型调用（商汤 SenseNova） | KEY 存于 gitignored `.env` |

未设置时对应测试自动跳过并打印说明；**DoD 要求设置后跑绿**。

## 架构总览

```
Product → Factory → Capability Asset → Agent → Runtime → Execution → Verification → Knowledge → Model Provider
```

| Crate | 职责 |
|---|---|
| `core/runtime`(forge-core) | 9 种强类型 ID、ForgeError |
| `core/session` | 会话事件追加存储 + 确定性 replay |
| `core/event` | broadcast 事件总线（topic 过滤） |
| `core/artifact` | 产物模型 + SHA-256 |
| `core/agent` | Agent trait + 回合引擎（循环/上限/中止三护栏）|
| `core/task` | 任务状态机 + 内建验收标准（空验收禁令）|
| `planning/*` | Planner / 顺序规划器 / DAG / **波次调度器** |
| `execution/runtime` | 工具路由 + 执行引擎（超时/权限）+ PermissionPolicy |
| `execution/sandbox` | DenyAll/AllowList/PolicyChain（默认只读）|
| `execution/workspace` | 托管工作目录（任务隔离 + 逃逸防护）|
| `verification/*` | Command/File 验证器 · 不可变证据链 · AllPass 门禁 |
| `recovery` | 失败分类 + 有界重试指数退避引擎 |
| `capability/*` | 能力注册表 · Skill 加载(+信任校验) · MCP stdio · LLM 客户端+LlmAgent(商汤6.8/6.7优先) |
| `products/manifest` | ProductManifest / 模板实例化 / 产品装配 |
| `storage` | PostgreSQL(sqlx) 四 store + MinIO(S3 SigV4) 对象存储 |
| `server` | axum HTTP API，env 驱动存储切换 |
| `sdk` | ForgeSdk 门面（一行组装内存/PG栈）|
| `tools/worklog` | 多 AI 协作记录管理 CLI（JSON 事实源） |
| `cli` | `forge` 命令入口（version / serve）|

## 核心纪律

1. **验收驱动**：任务完成 = DoD 命令实际通过；禁止弱化断言。
2. **验证即完成条件**：Gate 不放行 → 任务不得 Completed；空验收禁令双层设防。
3. **能力先注册后使用**；权限默认只读，高风险需显式放行。
4. **秘密不落地**：API KEY 仅存 gitignored `.env`。

## 多 AI 协作

**任何 AI 开工前必读**：[`AI_WORKFLOW.md`](AI_WORKFLOW.md)（规范 v1.1），
然后 [`HANDOFF.md`](HANDOFF.md) → `PROGRESS.md` → `WORKLOG.md` 近几条。
状态更新一律走 `forge-worklog` 命令 + `export`（Markdown 是生成视图勿手改）。

## 文档导航

- [docs/architecture.md](docs/architecture.md) —— 分层与数据流详解
- build_a/b/c.md —— 第一阶段施工包（24 任务包规格）
- WORKLOG R6/R7 —— 全部架构决策与偏差记录
