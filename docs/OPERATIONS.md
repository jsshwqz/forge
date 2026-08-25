# 运维手册

## 1. 部署形态

| 形态 | 入口 | 适用 |
|---|---|---|
| 三件套（推荐） | `deploy/docker-compose.yml`（server+PG+MinIO） | 生产/演示 |
| 裸二进制 | `target/release/forge-server` + 外部 PG | 定制环境 |
| 内存模式 | 不配 `FORGE_PG_URL` | 本地开发（重启即清空） |

## 2. 环境变量清单

见《deploy/README.md》配置清单表。变更后重启进程生效；无热加载。

## 3. 备份与恢复

```bash
# 备份（PG 数据卷内数据）
podman exec <pg容器> pg_dump -U postgres forge > backup_$(date +%F).sql
# 恢复
cat backup.sql | podman exec -i <pg容器> psql -U postgres forge
```

MinIO 卷 `miniodata` 按对象存储常规方式快照。**备份节奏建议：每日全量。**

## 4. 升级路径

迁移在 server 启动时自动幂等执行（advisory lock 已防并发竞态）。
标准升级 = 新镜像替换 + 重启；回滚 = 换回旧镜像重启（向前兼容由 sqlx 迁移保证，
禁止删除已发布迁移）。

## 5. 健康与监控

- 存活：`GET /health`（免鉴权，接 LB 探针）
- 指标：`GET /metrics` 抓取五计数器；`verifications_fail` 持续上涨 = 任务质量劣化信号
- 日志：stdout 结构化文本；容器场景接 `docker logs`/journal

## 6. 故障排查速查

| 现象 | 排查 |
|---|---|
| 进程启动即退出并提示 SEC-001 | 非 loopback 监听未配 `FORGE_API_KEY` |
| 全部请求 401 | 客户端未带/带错 Bearer；确认 key 与服务端一致 |
| orchestrate 卡满 timeout | 工具执行超时（`OrchestrateRequest.timeout_secs`），检查工具依赖 |
| LLM 调用报 llm http 429/5xx 后自愈 | 供应商限流/抖动，客户端已自动退避三次；持续失败检查配额 |
| 重启后产品实例列表为空 | 实例存储为内存 MVP（R7-008）；任务/会话/证据在 PG 中不受影响 |

## 7. 性能与容量

- 单机内存模式下千级任务无压力；PG 模式瓶颈在数据库连接池（max=5，可调）；
- SSE 长连接数受 OS 文件句柄限制，生产建议置于反向代理之后。
