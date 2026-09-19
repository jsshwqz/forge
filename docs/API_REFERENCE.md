# API 参考（V6.0）

基址：`http://<host>:<port>`；启用 `FORGE_API_KEY` 后，除 `GET /health`
外所有请求需带 `Authorization: Bearer <key>`。401 为统一文案，不回显密钥。

错误格式统一：`{"error":{"code":"...","message":"..."}}`；
状态码语义：404 不存在 · 409 状态冲突/重复 · 422 请求体不合法 · 500 内部。

## 系统

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 存活探针（免鉴权） |
| GET | `/metrics` | Prometheus 文本计数器 |

## 任务

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/tasks` | `{goal, constraints[], acceptance[]}`；acceptance 元素含 `id/description/check`，check 三选一：`{"Command":"..."}` / `{"FileContains":{path,needle}}` / `{"FileExists":"path"}` |
| GET | `/tasks` | 列表（只读） |
| GET | `/tasks/:id` | 详情 |

## 编排

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/orchestrate` | `{goal, timeout_secs, acceptance[]}` → 计划→执行→验收→证据→门禁一次跑完；响应含 `final_status/gate_passed/steps_completed/evidence_ids/replans_used/escalated_to_human/plan_versions` |

## 会话与证据

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/sessions/:id` | 会话对象（events 数组即时间线） |
| GET | `/events/stream` | SSE 实时事件流（keep-alive 15s） |
| POST | `/api/evidence` | 手工补录证据 `{criterion_id,content,produced_by}` |
| GET | `/api/evidence/:id` | 证据详情 |

## 产品工厂（V4.0）

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/templates` | 发布模板 `{template,version,review_verdict}`；verdict 仅接受 `"Pass"/"Concern"`（衔接 V3.2 Reviewer） |
| GET | `/templates` | 模板列表 |
| POST | `/products/instantiate` | `{template_id,version,name?,params{}}` → Draft 实例 |
| GET | `/products` | 实例列表 |
| GET | `/products/:id` | 实例详情（state） |
| POST | `/products/:id/start` | Draft/Stopped → Active |
| POST | `/products/:id/stop` | Active → Stopped |
| POST | `/products/:id/deprecate` | Stopped/Draft → Deprecated（终态） |

### 示例：发布并实例化

```bash
curl -X POST :8080/templates -d '{
  "template": {"id":"tpl.demo","name":"Demo","parameters":[],
    "manifest_skeleton":{"id":"product_x","name":"demo","version":"1.0.0",
      "description":"","capabilities":[],
      "entry_agent_role":"Orchestrator"}},
  "version":"1.0.0","review_verdict":"Pass"}'

curl -X POST :8080/products/instantiate \
  -d '{"template_id":"tpl.demo","version":"1.0.0"}'
```

## 知识（KNW，V4.0）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/knowledge/failures` | 失败知识库列表（FailureRecord 聚合） |
| GET | `/knowledge/sessions/:id/export` | 会话回放导出（JSON 归档） |

## 能力市场（MKT，V5.0/V6.0）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/market/capabilities` | 公开只读能力目录（免鉴权） |
| GET | `/market/templates` | 已发布模板目录（免鉴权） |
| POST | `/market/install` | 安装能力（需鉴权；钉 yanked 版本 → 409，`*` 解析跳过 yanked → 404） |
| POST | `/market/publish` | 发布 release（需鉴权 + 发布者签名验签；未登记 publisher → 403） |
| POST | `/market/review` | 审核 release（pending→approved 自动转 published / pending→rejected 终态；终态再审 → 409） |
| GET | `/market/releases` | release 版本列表（隐藏 yanked） |

## 计费与用量（BILL，admin 面）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/admin/usage` | 计量三维度流水查询（按租户/用途聚合） |
| POST | `/admin/rates` | 设置费率（幂等，费率表） |
| GET | `/admin/rates` | 列出费率表 |
| POST | `/admin/bills/generate` | 生成账单（幂等，按费率×用量） |
| GET | `/admin/bills/:id` | 账单详情 |
| GET | `/admin/bills/:id/export` | 账单导出（JSON/CSV） |

## 大模型配置（LLM）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/llm/config` | 当前配置视图 + 厂商预设列表（不回显完整 key） |
| POST | `/api/llm/config` | 热更新大模型配置（可选持久化至 forge.env） |
| POST | `/api/llm/test` | 在线连通性与模型探测测试 |

## 控制台（HTML）

| 路径 | 页面 |
|---|---|
| `/` | 任务列表 |
| `/ui/sessions` | 会话时间线（输入 Session ID） |
| `/ui/evidence` | 证据查看（输入 Evidence ID） |

## curl 包装器（启用鉴权后）

```bash
af() { curl -H "Authorization: Bearer $FORGE_API_KEY" "$@" ; }
```
