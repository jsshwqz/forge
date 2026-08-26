# 快速上手（15 分钟从零到第一个交付任务）
> **实测演练记录**：2026-08-27 · Windows + PostgreSQL@15432 · scripts/ga_acceptance.ps1 十三步全 PASS
> （证据：artifacts/ga_evidence_*.json）。清洁机口径：全新 clone + 空数据库卷。


> 交钥匙试锁标准：一台从未接触过本项目的干净机器，按本文一次跑通。

## 前置（约 5 分钟）

- Rust 1.94+（`rustup` 默认工具链）
- PostgreSQL 可选——不配也能跑（内存模式，重启即清空）

## 第 1 步：构建并启动（约 6 分钟首次编译）

```bash
git clone <repo> && cd aion-forge
cargo build -p forge-server
FORGE_PORT=8080 cargo run -p forge-server
# 看到 "forge-server listening on http://127.0.0.1:8080" 即成功
```

## 第 2 步：健康检查 + 创建任务（1 分钟）

```bash
curl http://127.0.0.1:8080/health
curl -X POST http://127.0.0.1:8080/tasks \
  -H 'Content-Type: application/json' \
  -d '{"goal":"输出 Hello AionForge","acceptance":[]}'
```

## 第 3 步：跑一个真实交付任务（2 分钟）

```bash
curl -X POST http://127.0.0.1:8080/orchestrate \
  -H 'Content-Type: application/json' \
  -d '{
    "goal": "留下问候文件",
    "timeout_secs": 30,
    "acceptance": [{"id":"AC-1","description":"生成 out.txt",
                    "check":{"Command":"echo hello> out.txt"}}]
  }'
```

响应中 `final_status=Completed`、`gate_passed=true`、`evidence_count>=1`
即完成一次「计划→执行→验收→证据→门禁」闭环。

## 第 4 步：看控制台与指标（1 分钟）

- 浏览器打开 `http://127.0.0.1:8080/` —— 任务列表 / 会话时间线 / 证据查看
- `curl http://127.0.0.1:8080/metrics` —— Prometheus 计数器

## 下一步

- 接入真实 LLM 规划/审查：设置 `FORGE_LLM_BASE_URL`、`FORGE_LLM_API_KEY`
- 生产部署（Docker 三件套）：见 `deploy/README.md`
- 全部端点：见《API 参考》（docs/API_REFERENCE.md）
