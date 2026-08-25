# 部署说明（PKG-001）

## 一键起（三件套）

```bash
cp deploy/.env.example deploy/.env   # 填写 FORGE_API_KEY
docker compose -f deploy/docker-compose.yml up -d
# 或: podman compose -f deploy/docker-compose.yml up -d
curl http://localhost:8080/health
```

组成：`forge-server`（本目录 Dockerfile 构建）+ PostgreSQL 16 + MinIO。

## 配置清单（全部环境变量）

| 变量 | 必填 | 说明 |
|---|---|---|
| `FORGE_API_KEY` | 对外暴露时**必填** | Bearer 密钥；SEC-001：非 loopback 监听未配置将拒绝启动 |
| `FORGE_HOST` / `FORGE_PORT` | 否 | 监听地址，默认 127.0.0.1:8080 |
| `FORGE_PG_URL` | 持久化必填 | 如 compose 内 `postgres://postgres:forge@pg:5432/forge`；不配则内存模式（重启即失） |
| `FORGE_CORS_ORIGINS` | 否 | 逗号分隔白名单；留空=关闭 CORS |
| `FORGE_S3_ENDPOINT/ACCESS_KEY/SECRET_KEY/BUCKET` | 否 | MinIO/S3 工件存储 |
| `FORGE_LLM_BASE_URL`/`FORGE_LLM_API_KEY` | LLM 功能必填 | 商汤等 OpenAI 兼容端点 |
| `FORGE_TIER_HIGH_MODEL`/`FORGE_TIER_LOW_MODEL` | 否 | 成本分层路由（Low 缺省回落 High） |

## 数据持久化

- 卷 `pgdata`：任务/会话/证据全部关系数据 —— **备份这个卷即可**。
- 卷 `miniodata`：工件对象存储。
- 备份示例：`podman exec deploy-pg-1 pg_dump -U postgres forge > backup.sql`

## 升级路径

迁移在 server 启动时幂等执行（`connect_and_migrate`，含 advisory lock 防并发）。
升级 = 替换镜像 + 重启容器，无需手工跑 SQL：

```bash
git pull && docker compose -f deploy/docker-compose.yml build server
docker compose -f deploy/docker-compose.yml up -d server
```

## 安全基线（SEC-001 落实）

- 非 loopback 监听未配 `FORGE_API_KEY` → 进程拒绝启动（代码强制，非告警）。
- CORS 默认完全关闭；需要浏览器跨域时显式设置 `FORGE_CORS_ORIGINS`。
- 除 `/health` 外所有路由要求 `Authorization: Bearer <key>`；401 不回显任何密钥材料。
