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

## 压测脚本

脚本位置：deploy/bench/orch_load.ps1

用法：
```powershell
pwsh deploy/bench/orch_load.ps1 -Concurrent 50 -Duration 300
```

输出：RPS / p95 延迟 / 错误率

---

## 回归判定

- 与基线对比，任何基准项性能下降 >30% → 记 R7 上报
- 压测 RPS 下降 >30% → 记 R7 上报
