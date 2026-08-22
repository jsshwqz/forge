# Aion Forge 2.0 · 任务状态索引（单一事实源）

> 维护规则：每个任务结束后更新本表。状态枚举：⬜ 未开始 / 📌 WIP / ✅ 完成 / ❌ 失败 / 🚧 阻塞。
> 对应规范：`AI_WORKFLOW.md`

## 第一阶段（MVP）—— 已完成 ✅

| 任务 ID | 名称 | 状态 | owner | 最近记录 | 提交 |
|---|---|---|---|---|---|
| AF-CORE-001 | Workspace 与基础库 | ✅ 完成 | builder-a | WORKLOG R1-001 | 5b71095 |
| AF-CORE-002 | Session 模型 | ✅ 完成 | builder-a | WORKLOG R1-002 | 4ff32e9 |
| AF-CORE-003 | 事件总线 | ✅ 完成 | builder-a | WORKLOG R1-003 | 784ca40 |
| AF-CORE-004 | Artifact 模型 | ✅ 完成 | builder-a | WORKLOG R1-004 | 304a31d |
| AF-CORE-005 | Session 回放 | ✅ 完成 | builder-a | WORKLOG R1-005 | 83f7692 |
| AF-AGENT-001 | Agent 接口 | ✅ 完成 | builder-a | WORKLOG R1-006 | 1a6421b |
| AF-AGENT-002 | 回合引擎 | ✅ 完成 | builder-a | WORKLOG R1-007 | 6a3fed9 |
| AF-TASK-001 | Task 模型 | ✅ 完成 | builder-a | WORKLOG R1-008 | f832f0b |
| AF-PLAN-001 | Planner 接口 | ✅ 完成 | builder-a | WORKLOG R1-009 | 990acee |
| AF-PLAN-002 | DAG 规划器 | ✅ 完成 | builder-a | WORKLOG R1-010 | c6d412c |
| AF-EXEC-001 | 工具路由 | ✅ 完成 | builder-a | WORKLOG R1-011 | 5123e8f |
| AF-EXEC-002 | 执行运行时 | ✅ 完成 | builder-a | WORKLOG R1-012 | 6be6b53 |
| AF-EXEC-003 | 权限策略 | ✅ 完成 | builder-a | WORKLOG R1-013 | 27bd638 |
| AF-VERIFY-001 | Verifier 接口 | ✅ 完成 | builder-a | WORKLOG R1-014 | - |
| AF-VERIFY-002 | 证据链 | ✅ 完成 | builder-a | WORKLOG R1-015 | - |
| AF-VERIFY-003 | 质量门禁 | ✅ 完成 | builder-a | WORKLOG R1-016 | - |
| AF-RECOVERY-001 | 失败分类 | ✅ 完成 | builder-a | WORKLOG R1-017 | - |
| AF-RECOVERY-002 | 恢复引擎 | ✅ 完成 | builder-a | WORKLOG R1-018 | - |
| AF-CAP-001 | 能力注册表 | ✅ 完成 | builder-a | WORKLOG R1-019 | - |
| AF-CAP-002 | Skill 加载器 | ✅ 完成 | builder-a | WORKLOG R1-020 | - |
| AF-CAP-003 | MCP 适配器 | ✅ 完成 | builder-a | WORKLOG R1-021 | - |
| AF-PRODUCT-001 | ProductManifest | ✅ 完成 | builder-a | WORKLOG R1-022 | - |
| AF-PRODUCT-002 | 产品模板 | ✅ 完成 | builder-a | WORKLOG R1-023 | - |
| AF-PRODUCT-003 | 产品装配 | ✅ 完成 | builder-a | WORKLOG R1-024 | - |

**里程碑**：M1 ✅ / M2 ✅ / M3 ✅ / M4 ✅ · 总验收通过（159 tests, clippy 0 warnings）

## 技术栈合规 —— 冻结项落地进度

| 任务 ID | 名称 | 状态 | owner | 最近记录 | 提交 |
|---|---|---|---|---|---|
| COMP-001 | CLI 接入 clap | ✅ 完成 | builder-a | WORKLOG R1-004 | 6396107 |
| COMP-002 | 集成 tracing 可观测 | ⬜ 未开始 | - | - | - |
| COMP-003 | Rust 版工作记录工具 | ✅ 完成 | builder-a | WORKLOG R1-003 | 28a3d13 后并入 b5b9c90 |
| DATA-001 | Markdown 数据迁移到 JSON（forge-worklog） | ⬜ 未开始 | - | - | - |

> 说明：COMP-003 已实现并验证（8 tests, clippy 0 warnings），`tools/worklog` 已加入 workspace。
> 环境已恢复：cargo 1.94.0 可用（已加入用户 PATH）。
> 剩余偏差：tracing 未落地（COMP-002）；anyhow 未使用（可接受，库代码用 thiserror）。

## 第二阶段（未开始）—— ⬜

| 任务 ID | 名称 | 状态 | 备注 |
|---|---|---|---|
| PH2-001 | PostgreSQL / MinIO 持久化接入 | ⬜ 未开始 | 需第二阶段施工包 |
| PH2-002 | axum HTTP server | ⬜ 未开始 | 需第二阶段施工包 |
| PH2-003 | MCP 完整协议 | ⬜ 未开始 | 需第二阶段施工包 |
| PH2-004 | 真实模型接入 | ⬜ 未开始 | 需第二阶段施工包 |
| PH2-005 | Skill 签名/完整性校验 | ⬜ 未开始 | 需第二阶段施工包 |
