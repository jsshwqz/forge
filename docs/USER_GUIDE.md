# 操作手册（用户指南）

## 1. 日常使用流程

```
发布模板(Pass) → 实例化 → start → orchestrate 真实任务 → 查看证据/时间线 → stop
```

1. **模板发布**：`POST /templates`，必须携带 V3.2 Reviewer 的 `Pass/Concern`
   裁决；`Reject` 的模板不允许入库（回炉重做）。
2. **实例化**：`POST /products/instantiate` 生成 `Draft` 实例。
3. **启停**：`start`（Draft/Stopped→Active）、`stop`（Active→Stopped）、
   `deprecate`（终态，不可复活）。非法迁移服务端返回 409。
4. **跑任务**：`POST /orchestrate`。响应字段：
   - `gate_passed`：验收门禁是否通过；
   - `replans_used`：消耗的重规划次数（ORCH-003）；
   - `escalated_to_human=true`：表示已升级人工，需人介入排查；
   - `evidence_ids`：逐条用 `GET /api/evidence/:id` 或控制台查看。

## 2. LLM 能力开关

| 目的 | 环境变量 |
|---|---|
| 接入真实模型 | `FORGE_LLM_BASE_URL` + `FORGE_LLM_API_KEY` |
| LLM 规划/重规划/审查 | 同上（流水线与 ORCH-003 自动使用） |
| 成本分层路由 | `FORGE_TIER_HIGH_MODEL` / `FORGE_TIER_LOW_MODEL`（Low 缺省回落 High 并告警一次） |

离线优先：未配置 LLM 时全部功能仍可运行（确定性规划器 + mock 场景），
供应商限流(429)与引擎抖动(5xx)由客户端自动退避重试。

## 3. 控制台

- `/` 任务列表：一键刷新；
- `/ui/sessions`：粘贴 Session ID 查看完整事件时间线（含成本事件 payload.cost）；
- `/ui/evidence`：粘贴 Evidence ID 查看验收原始输出。

## 4. 可观测

- `/metrics`：五计数器（tasks/executions/verifications_pass/fail/replans_total），
  Prometheus 抓取即可建面板；
- `/events/stream`：SSE 实时事件流，可用于自建告警/看板。

## 5. 失败知识库（KNW-001）

失败记录（分类+消息+是否可重试）与关联证据在编排过程中产生，
经 `forge-knowledge` 提供按 **category / 工具名 / 关键词** 三维检索，
Session 全量事件支持 JSON 归档导出（回放复盘）。当前为库形态 MVP，
HTTP 检索端点将在知识消费场景明确后开放。

## 6. 安全须知

- 对外监听必须设置 `FORGE_API_KEY`（否则进程拒绝启动）；
- CORS 默认关闭；确需浏览器跨域时用 `FORGE_CORS_ORIGINS` 白名单；
- 密钥仅经环境变量注入，不落日志不落库；401 响应统一文案。
