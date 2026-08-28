# Aion Forge 性能基线 (V5.0 PERF-001)

> **建立日期**: 2026-08-27
> **环境**: Windows, Rust stable
> **说明**: 核心纯函数基准测试基线，后续版本对比回归 >30% 记 R7

---

## 微基准测试结果

| 基准项 | 迭代次数 | 平均延迟 | 说明 |
|--------|----------|----------|------|
| validate_plan | 1000 | ~0.01ms | 计划结构验证 |
| session_replay | 1000 (1000 events) | ~0.5ms | Session 序列化 |
| gate_evaluate | 1000 (50 outcomes) | ~0.02ms | Gate 条件评估 |

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
