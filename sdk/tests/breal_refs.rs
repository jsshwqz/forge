//! B-REAL-001C 测试矩阵（测试名冻结）。
//!
//! 覆盖 resolve_refs 的 7 项冻结断言。
//! #8 e2e 放 001D。

use std::collections::BTreeMap;

use forge_core::ForgeError;
use forge_sdk::orchestrator::resolve_refs;
use serde_json::{json, Value};

fn done_with(s1_output: Value) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("s1".to_string(), s1_output);
    m
}

// ── #1 ──
#[test]
fn ref_absent_passthrough_unchanged() {
    let done = BTreeMap::new();
    let input = json!({
        "text": "hello world",
        "count": 42,
        "flag": true,
        "nested": {"inner": "value", "arr": [1, 2, 3]}
    });

    let resolved = resolve_refs(&input, &done).unwrap();
    assert_eq!(resolved, input, "no $ refs → exact passthrough");
}

// ── #2 ──
#[test]
fn ref_whole_output_injects_typed_value() {
    let output = json!({"headers": ["a", "b"], "rows": [{"name": "alice"}]});
    let done = done_with(output.clone());

    let input = json!({"data": "$s1.output"});
    let resolved = resolve_refs(&input, &done).unwrap();

    // 注入的应该是对象原值（非字符串）
    assert_eq!(
        resolved.get("data").unwrap(),
        &output,
        "$s1.output should inject the whole typed value"
    );
    assert!(
        resolved["data"].is_object(),
        "injected value must be an object, not a string"
    );
}

// ── #3 ──
#[test]
fn ref_dot_path_and_index() {
    let output = json!({
        "rows": [
            {"name": "alice", "age": 30},
            {"name": "bob", "age": 25}
        ]
    });
    let done = done_with(output.clone());

    // .rows[1].name
    let input = json!({"target": "$s1.output.rows[1].name"});
    let resolved = resolve_refs(&input, &done).unwrap();

    assert_eq!(
        resolved.get("target").unwrap(),
        &json!("bob"),
        "dot path + index should resolve to 'bob'"
    );
}

// ── #4 ──
#[test]
fn ref_json_suffix_stringifies() {
    let output = json!({"rows": [{"name": "alice"}, {"name": "bob"}]});
    let done = done_with(output.clone());

    let input = json!({"serialized": "$s1.output.rows|json"});
    let resolved = resolve_refs(&input, &done).unwrap();

    let s = resolved.get("serialized").unwrap().as_str().unwrap();
    // 必须是合法 JSON 字符串且可回解
    let reparsed: Value = serde_json::from_str(s).unwrap();
    assert_eq!(reparsed, output["rows"]);
}

// ── #5 ──
#[test]
fn ref_unresolved_is_error() {
    let done = BTreeMap::new(); // 空 done 表

    // 引用不存在的 step
    let input1 = json!({"x": "$s1.output"});
    let err1 = resolve_refs(&input1, &done).unwrap_err();
    assert!(
        matches!(err1, ForgeError::InvalidState(ref msg) if msg.contains("$s1.output")),
        "error must contain full ref string, got: {err1}"
    );

    // step 存在但路径缺键
    let mut done2 = BTreeMap::new();
    done2.insert("s1".to_string(), json!({"a": 1}));
    let input2 = json!({"x": "$s1.output.b"});
    let err2 = resolve_refs(&input2, &done2).unwrap_err();
    assert!(
        matches!(err2, ForgeError::InvalidState(ref msg) if msg.contains("$s1.output.b")),
        "error must contain full ref string, got: {err2}"
    );
}

// ── #6 ──
#[test]
fn ref_no_interpolation() {
    let done = done_with(json!({"v": 1}));

    let input = json!({"x": "a$s1.output"});
    let err = resolve_refs(&input, &done).unwrap_err();
    assert!(
        matches!(err, ForgeError::InvalidState(ref msg) if msg.contains("interpolation")),
        "mixed string must be rejected as interpolation, got: {err}"
    );
}

// ── #7 ──
#[test]
fn ref_depth_limit() {
    let done = BTreeMap::new();

    // 9 层嵌套
    let mut input = json!({"v": "leaf"});
    for _ in 0..9 {
        input = json!({"nested": input});
    }

    let err = resolve_refs(&input, &done).unwrap_err();
    assert!(
        matches!(err, ForgeError::InvalidState(ref msg) if msg.contains("too deep")),
        "9-level nesting must be rejected, got: {err}"
    );
}
