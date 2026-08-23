# 架构详解

> 配套 README.md；决策依据见 WORKLOG R6 条目。

## 1. 分层与依赖方向

```
                ┌────────────┐
                │  products  │  装配：manifest+模板 → 可运行产品
                └─────┬──────┘
                ┌─────▼──────┐
                │ capability │  registry / skill(+trust) / mcp / api(LLM)
                └─────┬──────┘
        ┌─────────────▼─────────────┐
        │  agent(接口) ← llm_agent  │  动作空间封闭；真实模型经 LlmBackend 注入
        └─────────────┬─────────────┘
   ┌──────────┐  ┌────▼─────┐  ┌───────────┐
   │ planning │→ │ execution│→ │verification│   Plan→Execute→Verify
   │ planner  │  │ runtime  │  │ verifier  │
   │ dag      │  │ sandbox  │  │ evidence  │
   └──────────┘  └────┬─────┘  └─────┬─────┘
                ┌─────▼──────┐  ┌────▼────┐
                │ recovery   │  │ gates   │  失败分类→重试/升级；AllPass 门禁
                └─────┬──────┘  └────┬────┘
                ┌─────▼──────────────▼────┐
                │ core: session/event/    │  事件追加式存储 · broadcast 总线
                │ task · artifact · runtime│
                └───────────┬─────────────┘
                ┌───────────▼─────────────┐
                │ storage (PG/MinIO)      │  trait 不变，实现可替换
                └─────────────────────────┘
```

依赖规则：上层依赖下层 trait，绝不反向；Core 不依赖 UI/存储实现。

## 2. 关键数据流

### 2.1 任务生命周期
```
Task(Pending) → Planner → Plan(DAG) → TurnEngine+Agent → ExecutionEngine
     → CheckSpec → Verifier → Evidence → Gate(AllPass) → Task(Completed)
```
- 任何失败 → classify → RecoveryEngine（重试≤3 指数退避 / 升级人工）→ 事件总线广播

### 2.2 会话事件
`SessionStore.append` 分配单调 seq 并按状态机迁移；`replay(events)` 纯函数重建状态
（PG 路径经 `pg_replay` 测试证明兼容）。

### 2.3 执行权限
`ToolRouter.route` → `PermissionPolicy.check(level, ctx)` → 超时包裹调用 → Session 双事件。
默认策略 AllowList[ReadOnly]；Irreversible 永远需显式放行。

## 3. 存储矩阵

| Trait | 内存实现 | PostgreSQL | MinIO(S3) |
|---|---|---|---|
| SessionStore | InMemory ✅ | PgSessionStore ✅ | — |
| TaskStore | InMemory ✅ | PgTaskStore ✅ | — |
| ArtifactStore | InMemory ✅ | PgArtifactStore ✅(BYTEA) | MinioArtifactStore ✅(SigV4) |
| EvidenceStore | InMemory ✅ | PgEvidenceStore ✅ | — |

切换方式：`server` main 按 `FORGE_PG_URL` 组装 State；trait 不变（AP-015）。

## 4. 可观测与安全

- tracing 埋点：exec 引擎（start/finish/denied/timeout）、recovery 决策、turn 汇总；
  订阅器由应用层安装（tracing-subscriber 待白名单扩批）。
- 秘密：仅环境变量 / gitignored `.env`。
- MCP：行分隔 JSON-RPC 2.0（2024-11-05）；按 id 匹配响应，服务端通知忽略；
  未知工具 -32601 映射 InvalidState。

## 5. 已知偏差（详见 WORKLOG R7）

| # | 偏差 | 状态 |
|---|---|---|
| R7-001 | route() 返回 Arc<dyn Tool>（规范 &dyn Tool）| 人工追认维持 |
| R7-002 | PermissionPolicy trait 在 forge-exec | 人工追认维持 |
| R7-003 | anyhow 未使用（库代码统一 thiserror）| 接受 |
