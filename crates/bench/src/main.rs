//! V5.0 PERF-001: 微基准测试
//! 
//! 三项纯函数基准：
//! 1. bench_validate_plan - 计划验证
//! 2. bench_session_replay - Session 回放 (1000事件)
//! 3. bench_gate_evaluate - Gate 评估 (50条 outcomes)

use std::time::Instant;

fn main() {
    println!("=== V5.0 PERF-001 微基准测试 ===");
    println!();
    
    // 基准 1: validate_plan
    bench_validate_plan();
    
    // 基准 2: session_replay
    bench_session_replay();
    
    // 基准 3: gate_evaluate
    bench_gate_evaluate();
    
    println!("
=== 基准测试完成 ===");
    println!("基线数据应写入 docs/BASELINE.md");
}

fn bench_validate_plan() {
    let iterations = 1000;
    let start = Instant::now();
    
    for _ in 0..iterations {
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
    }
    
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    println!("[bench_validate_plan] {} iter, avg {:.3}ms", iterations, avg_ms);
}

fn bench_session_replay() {
    let iterations = 1000;
    let events = (0..1000).map(|i| serde_json::json!({
        "kind": "TaskReceived",
        "data": format!("event-{}", i)
    })).collect::<Vec<_>>();
    
    let start = Instant::now();
    
    for _ in 0..iterations {
        // 模拟回放导出
        let _json = serde_json::to_string(&events).unwrap();
    }
    
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    println!("[bench_session_replay] {} iter (1000 events), avg {:.3}ms", iterations, avg_ms);
}

fn bench_gate_evaluate() {
    let iterations = 1000;
    let outcomes: Vec<serde_json::Value> = (0..50).map(|i| serde_json::json!({
        "criterion_id": format!("AC-{}", i),
        "verdict": if i % 3 == 0 { "Fail" } else { "Pass" },
        "reason": format!("reason-{}", i)
    })).collect();
    
    let start = Instant::now();
    
    for _ in 0..iterations {
        // 模拟 gate 评估
        let pass_count = outcomes.iter().filter(|o| o["verdict"] == "Pass").count();
        let _passed = pass_count == outcomes.len();
    }
    
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    println!("[bench_gate_evaluate] {} iter (50 outcomes), avg {:.3}ms", iterations, avg_ms);
}
