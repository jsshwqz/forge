# Aion Forge 性能基线 (V5.0 PERF-001)

> **建立日期**: 2026-08-27（V5-FIX-3 于 2026-09-05 补充 p50/p95/p99 百分位实测）
> **环境**: Windows, Rust stable（rustc 1.98.0）
> **说明**: 核心纯函数基准测试基线，后续版本对比回归 >30% 记 R7

---

## 微基准测试结果（1000 次逐次采样，forge-bench 实跑 2026-09-05）

| 基准项 | 迭代次数 | 平均延迟 | p50 | p95 | p99 | 说明 |
|--------|----------|----------|-----|-----|-----|------|
| validate_plan | 1000 | 0.0092ms | 0.0086ms | 0.0115ms | 0.0201ms | 计划结构验证 |
| session_replay | 1000 (1000 events) | 2.8634ms | 2.8527ms | 3.1011ms | 4.7098ms | Session 序列化 |
| gate_evaluate | 1000 (50 outcomes) | 0.0069ms | 0.0065ms | 0.0070ms | 0.0131ms | Gate 条件评估 |

> 复跑方式：`cargo run -p forge-bench`（逐次采样 + 排序取 p50/p95/p99）。

---

## 压测四数（G-V5 门禁项；2026-09-06 净库实测）

| 靶路径 | 并发 | 请求数 | RPS | p50 | p95 | 错误率 |
|--------|------|--------|-----|-----|-----|--------|
| POST /tasks（API 面，真实 PG） | 10 | 7410 | **493.7** | 16.81ms | 29.91ms | **0%** |
| POST /orchestrate（全链路：计划→echo→命令验收→门禁→证据→PG） | 1 | 40 | **8.2** | 117.05ms | 140.43ms | **0%** |

> 环境同上（rustc 1.98.0 + Podman forge-pg 容器）。证据文件：`artifacts/load_last.json`。
> **注意**：/orchestrate 受 TEN-003 配额门控（默认并发 4 / 日 100）——压测该路径须净库 +
> 低并发限量，否则 429 属配额生效（本表即为净库口径）。

脚本位置：deploy/bench/orch_load.ps1

用法：
```powershell
pwsh deploy/bench/orch_load.ps1 -Concurrent 10 -DurationSec 15          # API 面
MSYS_NO_PATHCONV=1 pwsh deploy/bench/orch_load.ps1 -Path /orchestrate `
  -Body '<json>' -Concurrent 1 -MaxRequests 40                          # 全链路（Git Bash 需前缀变量）
```

---

## 回归判定

- 与基线对比，任何基准项性能下降 >30% → 记 R7 上报
- 压测 RPS 下降 >30% → 记 R7 上报
