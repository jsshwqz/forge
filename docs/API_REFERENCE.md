# API 参考（V4.0）

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
