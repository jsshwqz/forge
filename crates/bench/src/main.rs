//! V5.0 PERF-001: 微基准测试
//!
//! 三项纯函数基准：
//! 1. bench_validate_plan - 计划验证
//! 2. bench_session_replay - Session 回放 (1000事件)
//! 3. bench_gate_evaluate - Gate 评估 (50条 outcomes)
//!
//! V5-FIX-3：逐次采样并输出 p50/p95/p99 百分位（此前仅平均值）。

use std::time::{Duration, Instant};

/// 逐次采样运行 `work`，输出平均与 p50/p95/p99（毫秒）。
fn bench_percentiles(name: &str, note: &str, iterations: usize, mut work: impl FnMut()) {
    let mut samples: Vec<Duration> = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        work();
        samples.push(start.elapsed());
    }
    samples.sort();
    let pct = |p: f64| -> f64 {
        let idx = ((p / 100.0) * (samples.len() as f64 - 1.0)).round() as usize;
        samples[idx].as_secs_f64() * 1000.0
    };
    let avg = samples.iter().sum::<Duration>().as_secs_f64() * 1000.0 / samples.len() as f64;
    println!(
        "[{name}] {} iter{}, avg {:.4}ms | p50 {:.4}ms | p95 {:.4}ms | p99 {:.4}ms",
        iterations, note, avg, pct(50.0), pct(95.0), pct(99.0)
    );
}

fn main() {
    println!("=== V5.0 PERF-001 微基准测试 ===");
    println!();

    // 基准 1: validate_plan
    bench_validate_plan();

    // 基准 2: session_replay
    bench_session_replay();

    // 基准 3: gate_evaluate
    bench_gate_evaluate();

    println!();
    println!("=== 基准测试完成 ===");
    println!("基线数据应写入 docs/BASELINE.md");
}

fn bench_validate_plan() {
    bench_percentiles("bench_validate_plan", "", 1000, || {
        // 模拟计划验证逻辑
        let plan_json = serde_json::json!({
            "id": "plan-test",
            "task_id": "task-test",
            "steps": [
                {"id": "step1", "title": "test", "depends_on": [], "action": {"type": "echo", "input": "hello"}}
            ],
            "status": "ready"
        });

        // 简单验证
        let _valid = plan_json["steps"].is_array() && plan_json["status"] == "ready";
    });
}

fn bench_session_replay() {
    let events = (0..1000).map(|i| serde_json::json!({
        "kind": "TaskReceived",
        "data": format!("event-{}", i)
    })).collect::<Vec<_>>();

    bench_percentiles("bench_session_replay", " (1000 events)", 1000, || {
        // 模拟回放导出
        let _json = serde_json::to_string(&events).unwrap();
    });
}

fn bench_gate_evaluate() {
    let outcomes: Vec<serde_json::Value> = (0..50).map(|i| serde_json::json!({
        "criterion_id": format!("AC-{}", i),
        "verdict": if i % 3 == 0 { "Fail" } else { "Pass" },
        "reason": format!("reason-{}", i)
    })).collect();

    bench_percentiles("bench_gate_evaluate", " (50 outcomes)", 1000, || {
        // 模拟 gate 评估
        let pass_count = outcomes.iter().filter(|o| o["verdict"] == "Pass").count();
        let _passed = pass_count == outcomes.len();
    });
}
