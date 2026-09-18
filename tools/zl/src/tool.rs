//! zl 工具集：P0/P1 感知与策略层工具（9 个，纯计算 / 零依赖 / ReadOnly）。
//!
//! 清单：check_sufficiency / verify_result / compile_contract / detect_drift /
//! contradiction_analyze / prompt_audit / verify_contract / strategic_plan /
//! evolver_governance。
//!
//! 代码模式严格照抄 forge 既有工具集（tools/text 等）：
//! `Tool` trait 来自 forge-exec（execution/runtime/src/router.rs）；
//! `ForgeError / ForgeResult` 来自 forge-core（core/runtime/src/error.rs）；
//! 单字段 descriptor struct 模式；`register_all` 逐个 register。
//! 全部工具只有 `ReadOnly` 权限，不访问外部资源，输入 JSON → 纯计算 → 输出 JSON。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::{json, Value};

fn err(msg: impl Into<String>) -> ForgeError {
    ForgeError::InvalidState(msg.into())
}

// ── 公共 criteria 比对（verify_result / detect_drift 共享，按叶子路径逐项比对） ──

/// 递归收集 JSON 的全部叶子路径。路径形如 `a.b[0].c`（根标量返回 ["$"]）。
fn collect_leaf_paths(value: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for k in keys {
                    let child = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                    walk(&map[k], &child, out);
                }
            }
            Value::Array(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    let child = format!("{path}[{i}]");
                    walk(item, &child, out);
                }
            }
            _ => out.push(if path.is_empty() { "$".to_string() } else { path.to_string() }),
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

/// 按叶子路径取值（支持 `a.b[0].c`）。路径由 `collect_leaf_paths` 生成，格式确定。
fn get_at_path<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut cur = value;
    let mut buf = String::new();
    let mut chars = path.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '[' => {
                chars.next();
                let mut idx = String::new();
                while let Some(&d) = chars.peek() {
                    if d == ']' {
                        chars.next();
                        break;
                    }
                    idx.push(d);
                    chars.next();
                }
                if let Ok(i) = idx.parse::<usize>() {
                    if let Value::Array(arr) = cur {
                        if i < arr.len() {
                            cur = &arr[i];
                        }
                    }
                }
            }
            '.' => {
                chars.next();
            }
            _ => {
                buf.clear();
                while let Some(&d) = chars.peek() {
                    if d == '.' || d == '[' {
                        break;
                    }
                    buf.push(d);
                    chars.next();
                }
                if let Value::Object(map) = cur {
                    if let Some(v) = map.get(&buf) {
                        cur = v;
                    }
                }
            }
        }
    }
    cur
}

/// 期望与实际的逐项比对：返回 (全部叶子条目, 命中数)。
fn compare_criteria(expected: &Value, actual: &Value) -> (Vec<(String, bool, Value)>, usize) {
    let leaves = collect_leaf_paths(expected);
    if leaves.is_empty() {
        let ok = expected == actual;
        return (vec![("$".to_string(), ok, if ok { json!("matches expected") } else { actual.clone() })], if ok { 1 } else { 0 });
    }
    let mut rows = Vec::new();
    let mut met = 0usize;
    for p in &leaves {
        let ev = get_at_path(expected, p);
        let av = get_at_path(actual, p);
        let ok = ev == av;
        if ok {
            met += 1;
        }
        rows.push((
            p.clone(),
            ok,
            if ok { json!("matches expected") } else { av.clone() },
        ));
    }
    (rows, met)
}

// ── check_sufficiency：上下文充分性感知（对齐原版 SYSTEM_SUFFICIENCY 字段） ──

pub struct CheckSufficiencyTool {
    descriptor: ToolDescriptor,
}

impl CheckSufficiencyTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "check_sufficiency".into(),
                description: "判断给定需求/任务是否有足够上下文与资源支持。输入 {task, requirements?, resources?}，返回 {sufficient, confidence, missing, recommendation}（原版字段，AI 判断降级为规则）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string", "description": "待评估任务/契约描述（文本）" },
                        "requirements": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "resource_type": { "type": "string" },
                                    "amount": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["id", "resource_type"]
                            }
                        },
                        "resources": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "resource_type": { "type": "string" },
                                    "status": { "type": "string" }
                                },
                                "required": ["id", "resource_type"]
                            }
                        }
                    }
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for CheckSufficiencyTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CheckSufficiencyTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        // 结构化输入优先；缺失时降级为 task 文本评估。
        let requirements = input.get("requirements").and_then(|v| v.as_array());
        let resources = input.get("resources").and_then(|v| v.as_array());

        if let (Some(requirements), Some(resources)) = (requirements, resources) {
            // 可匹配资源索引池：status 缺失或非 unavailable/failed 视为可用。
            let mut pool: Vec<usize> = resources
                .iter()
                .enumerate()
                .filter(|(_, r)| match r.get("status").and_then(|v| v.as_str()) {
                    Some(s) => s != "unavailable" && s != "failed",
                    None => true,
                })
                .map(|(i, _)| i)
                .collect();

            let mut missing = Vec::new();
            let mut matched_requirements = 0usize;
            for req in requirements {
                let rid = req
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unnamed)");
                let want_type = req
                    .get("resource_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let amount = req.get("amount").and_then(|v| v.as_u64()).unwrap_or(1).max(1);

                let mut assigned = 0u64;
                pool.retain(|&idx| {
                    if assigned >= amount {
                        return true;
                    }
                    let r = &resources[idx];
                    let rtype = r.get("resource_type").and_then(|v| v.as_str()).unwrap_or("");
                    if rtype == want_type {
                        assigned += 1;
                        false
                    } else {
                        true
                    }
                });

                if assigned >= amount {
                    matched_requirements += 1;
                } else {
                    missing.push(format!(
                        "requirement '{rid}': need {amount} resource(s) of type '{want_type}', got {assigned}"
                    ));
                }
            }

            let sufficient = missing.is_empty();
            let total = requirements.len().max(1);
            let confidence = if sufficient {
                1.0
            } else {
                matched_requirements as f64 / total as f64
            };
            let recommendation = if sufficient {
                "proceed"
            } else if matched_requirements > 0 {
                "gather_more"
            } else {
                "clarify_with_user"
            };
            return Ok(json!({
                "sufficient": sufficient,
                "confidence": confidence,
                "missing": missing,
                "recommendation": recommendation,
            }));
        }

        // 纯 task 文本：无法可靠判定充分性 → 诚实降级。
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let trimmed = task.trim();
        if trimmed.is_empty() {
            return Ok(json!({
                "sufficient": false,
                "confidence": 0.0,
                "missing": ["no task or requirements provided"],
                "recommendation": "clarify_with_user",
            }));
        }
        let snippet = truncate_mid(trimmed, 80);
        Ok(json!({
            "sufficient": false,
            "confidence": 0.3,
            "missing": [format!("insufficient structured context to evaluate: {snippet}")],
            "recommendation": "gather_more",
        }))
    }
}

/// 截断长文本到 max 字符（保留 UTF-8 边界）。
fn truncate_mid(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().skip(count - half).collect();
    format!("{head}...{tail}")
}

// ── verify_result：结果验证传感器（对齐原版 SYSTEM_VERIFY 字段） ──

pub struct VerifyResultTool {
    descriptor: ToolDescriptor,
}

impl VerifyResultTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "verify_result".into(),
                description: "检查执行结果是否满足契约/期望。输入 {expected, actual}（或 {criteria, actual}），返回 {passed, score, criteria_results, verdict, feedback}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expected": {},
                        "actual": {},
                        "criteria": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["expected", "actual"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for VerifyResultTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for VerifyResultTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let expected = input
            .get("expected")
            .ok_or_else(|| err("expected is required"))?;
        let actual = input
            .get("actual")
            .ok_or_else(|| err("actual is required"))?;

        let (rows, met) = compare_criteria(expected, actual);
        let total = rows.len().max(1);
        let score = met as f64 / total as f64;
        let passed = met == total;
        let verdict = if passed {
            "accept"
        } else if score >= 0.5 {
            "retry"
        } else {
            "escalate"
        };

        let criteria_results: Vec<Value> = rows
            .iter()
            .map(|(path, ok, evidence)| json!({
                "criterion": path,
                "met": ok,
                "evidence": evidence,
            }))
            .collect();

        let unmet: Vec<&str> = rows.iter().filter(|(_, ok, _)| !ok).map(|(p, _, _)| p.as_str()).collect();
        let feedback = if passed {
            format!("all {total} criteria met")
        } else {
            format!("{met} of {total} criteria met; unmet: {}", unmet.join(", "))
        };

        Ok(json!({
            "passed": passed,
            "score": score,
            "criteria_results": criteria_results,
            "verdict": verdict,
            "feedback": feedback,
        }))
    }
}

// ── compile_contract：任务契约编译器（对齐原版 SYSTEM_COMPILE_CONTRACT 字段） ──

pub struct CompileContractTool {
    descriptor: ToolDescriptor,
}

impl CompileContractTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "compile_contract".into(),
                description: "把自然语言任务编译为结构化契约。输入 {task, criteria?}，返回 {task_summary, acceptance_criteria, expected_outputs, required_context, verification_method, complexity, estimated_steps}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string", "description": "自然语言任务描述" },
                        "criteria": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "显式验收标准（可选，缺省从 task 拆句推导）"
                        }
                    },
                    "required": ["task"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }

    /// 按句子切分（中英文句号/分号/换行）。
    fn split_sentences(text: &str) -> Vec<String> {
        text.split(['。', '！', '？', '.', '!', '?', ';', '；', '\n'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    fn infer_output_type(task: &str) -> &'static str {
        let lower = task.to_lowercase();
        if lower.contains("code") || lower.contains("函数") || lower.contains("脚本") || lower.contains("实现") || lower.contains("rust") || lower.contains("python") {
            "code"
        } else if lower.contains("json") || lower.contains("数据") || lower.contains("列表") || lower.contains("返回") || lower.contains("report") || lower.contains("结果") {
            "data"
        } else {
            "text"
        }
    }
}

impl Default for CompileContractTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CompileContractTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("task is required"))?;
        let trimmed = task.trim();
        if trimmed.is_empty() {
            return Err(err("task must not be empty"));
        }

        // task_summary：一行规范化（折叠空白，截断 120 字符）。
        let normalized: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        let task_summary: String = normalized.chars().take(120).collect();

        // acceptance_criteria：显式 criteria 优先；否则拆句。
        let acceptance_criteria: Vec<String> = if let Some(criteria) = input.get("criteria").and_then(|v| v.as_array()) {
            criteria
                .iter()
                .filter_map(|c| c.as_str().map(|s| s.to_string()))
                .collect()
        } else {
            let sentences = Self::split_sentences(trimmed);
            if sentences.is_empty() {
                vec![format!("{task_summary} is completed successfully")]
            } else {
                sentences
            }
        };

        // expected_outputs：按关键词推断产出类型。
        let expected_outputs = vec![json!({
            "type": Self::infer_output_type(trimmed),
            "description": "",
        })];

        // required_context：占位符 {{x}} 视为外部上下文需求；否则未知。
        let mut required_context = Vec::new();
        let mut rest = trimmed;
        while let Some(pos) = rest.find("{{") {
            let after = &rest[pos + 2..];
            if let Some(close) = after.find("}}") {
                let name = after[..close].trim();
                if !name.is_empty() {
                    required_context.push(format!("{{{name}}}"));
                }
                rest = &after[close + 2..];
            } else {
                break;
            }
        }
        if required_context.is_empty() {
            required_context.push("unknown".to_string());
        }

        // verification_method。
        let lower = trimmed.to_lowercase();
        let verification_method = if lower.contains("test") || lower.contains("测试") || lower.contains("验证") || lower.contains("验收") {
            "run tests against acceptance criteria".to_string()
        } else {
            "unknown".to_string()
        };

        // complexity：句子数与长度综合。
        let char_len = trimmed.chars().count();
        let complexity = if char_len > 200 || lower.contains("complex") || lower.contains("复杂") {
            "high"
        } else if char_len > 80 || acceptance_criteria.len() > 3 {
            "medium"
        } else {
            "low"
        };

        // estimated_steps：句子数 clamp 1..10。
        let estimated_steps = acceptance_criteria.len().clamp(1, 10);

        Ok(json!({
            "task_summary": task_summary,
            "acceptance_criteria": acceptance_criteria,
            "expected_outputs": expected_outputs,
            "required_context": required_context,
            "verification_method": verification_method,
            "complexity": complexity,
            "estimated_steps": estimated_steps,
        }))
    }
}

// ── detect_drift：执行漂移传感器（对齐原版 SYSTEM_DRIFT 字段） ──

pub struct DetectDriftTool {
    descriptor: ToolDescriptor,
}

impl DetectDriftTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "detect_drift".into(),
                description: "对比当前状态与原始契约，检测离题漂移。输入 {expected, actual}，返回 {on_track, drift_score, drift_description, correction}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expected": { "type": "object", "description": "原始契约/期望状态" },
                        "actual": { "type": "object", "description": "当前实际状态" }
                    },
                    "required": ["expected", "actual"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for DetectDriftTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DetectDriftTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let expected = input
            .get("expected")
            .ok_or_else(|| err("expected is required"))?;
        let actual = input
            .get("actual")
            .ok_or_else(|| err("actual is required"))?;

        let (rows, met) = compare_criteria(expected, actual);
        let total = rows.len().max(1);
        let on_track = met == total;
        let drift_score = if total == 0 { 0.0 } else { 1.0 - met as f64 / total as f64 };

        let unmet: Vec<&str> = rows.iter().filter(|(_, ok, _)| !ok).map(|(p, _, _)| p.as_str()).collect();
        let drift_description = if on_track {
            "no drift detected".to_string()
        } else if unmet.is_empty() {
            "state differs from contract".to_string()
        } else {
            format!("{} field(s) drifted: {}", unmet.len(), unmet.join(", "))
        };

        let correction = if on_track {
            String::new()
        } else {
            unmet
                .iter()
                .map(|p| format!("restore {p} to expected"))
                .collect::<Vec<_>>()
                .join("; ")
        };

        Ok(json!({
            "on_track": on_track,
            "drift_score": drift_score,
            "drift_description": drift_description,
            "correction": correction,
        }))
    }
}

// ── contradiction_analyze：矛盾分析（对齐原版 SYSTEM_CONTRADICTION 字段） ──

pub struct ContradictionAnalyzeTool {
    descriptor: ToolDescriptor,
}

impl ContradictionAnalyzeTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "contradiction_analyze".into(),
                description: "分解任务，识别主要矛盾与瓶颈。输入 {constraints:[{id,field,op,value}]}（op ∈ gt/lt/gte/lte/eq/neq）或 {task}，返回 {contradictions, principal_contradiction, recommended_focus, resource_allocation}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string" },
                        "constraints": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "field": { "type": "string" },
                                    "op": { "type": "string", "enum": ["gt", "lt", "gte", "lte", "eq", "neq"] },
                                    "value": { "type": "number" }
                                },
                                "required": ["id", "field", "op", "value"]
                            }
                        }
                    }
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }

    /// 判断 `value` 是否满足 `op` 对比 `bound`。
    fn op_satisfies(op: &str, value: f64, bound: f64) -> bool {
        match op {
            "gt" => value > bound,
            "gte" => value >= bound,
            "lt" => value < bound,
            "lte" => value <= bound,
            "eq" => (value - bound).abs() < f64::EPSILON,
            "neq" => (value - bound).abs() >= f64::EPSILON,
            _ => true,
        }
    }

    /// 检查一对约束是否矛盾，返回矛盾理由。
    fn contradiction_reason(a: (&str, &str, f64), b: (&str, &str, f64)) -> Option<String> {
        let (op_a, _, va) = a;
        let (op_b, _, vb) = b;
        match (op_a, op_b) {
            ("eq", "eq") => {
                if (va - vb).abs() >= f64::EPSILON {
                    Some(format!("eq {va} conflicts with eq {vb}"))
                } else {
                    None
                }
            }
            ("eq", _other) | (_other, "eq") => {
                let eq_op = if op_a == "eq" { op_a } else { op_b };
                let bound_op = if op_a == "eq" { op_b } else { op_a };
                let eq_val = if op_a == "eq" { va } else { vb };
                let bound_val = if op_a == "eq" { vb } else { va };
                if !Self::op_satisfies(bound_op, eq_val, bound_val) {
                    Some(format!("{eq_op} {eq_val} violates {bound_op} {bound_val}"))
                } else {
                    None
                }
            }
            ("neq", "neq") => None,
            (a_low, b_up) => {
                let (low_op, low_val, up_op, up_val) =
                    if Self::is_lower(a_low) && Self::is_upper(b_up) {
                        (a_low, va, b_up, vb)
                    } else if Self::is_lower(b_up) && Self::is_upper(a_low) {
                        (b_up, vb, a_low, va)
                    } else {
                        return None;
                    };
                let low_strict = low_op == "gt";
                let up_strict = up_op == "lt";
                if low_val > up_val {
                    Some(format!("lower bound {low_val} exceeds upper bound {up_val}"))
                } else if (low_val - up_val).abs() < f64::EPSILON && (low_strict || up_strict) {
                    Some(format!("bounds {low_val}=={up_val} are both strict ({low_op}/{up_op})"))
                } else {
                    None
                }
            }
        }
    }

    fn is_lower(op: &str) -> bool {
        op == "gt" || op == "gte" || op == "eq"
    }

    fn is_upper(op: &str) -> bool {
        op == "lt" || op == "lte"
    }

    /// 文本级对立词检测（无结构化约束时的降级）。
    fn text_contradictions(task: &str) -> Vec<(String, String)> {
        let lower = task.to_lowercase();
        let mut out = Vec::new();
        let oppositions: &[(&str, &[&str])] = &[
            ("must not", &["must", "must be", "必须", "一定要"]),
            ("禁止", &["必须", "允许", "可以"]),
            ("不要", &["必须", "一定要"]),
            ("do not", &["always", "must"]),
        ];
        for (ban, allows) in oppositions {
            if lower.contains(ban) {
                for allow in *allows {
                    if lower.contains(allow) {
                        out.push((
                            format!("'{ban}' contradicts '{allow}'"),
                            "text".to_string(),
                        ));
                        break;
                    }
                }
            }
        }
        out
    }
}

impl Default for ContradictionAnalyzeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ContradictionAnalyzeTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let mut contradictions = Vec::new();
        let mut principal: Option<String> = None;
        let mut focus = String::new();
        let mut allocation = serde_json::Map::new();

        if let Some(constraints) = input.get("constraints").and_then(|v| v.as_array()) {
            struct Parsed {
                field: String,
                op: String,
                value: f64,
            }
            let mut parsed = Vec::new();
            for c in constraints {
                let field = c
                    .get("field")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let op = c
                    .get("op")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| err("constraint.op is required"))?;
                let value = c
                    .get("value")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| err("constraint.value must be a number"))?;
                parsed.push(Parsed {
                    field: field.to_string(),
                    op: op.to_string(),
                    value,
                });
            }

            for i in 0..parsed.len() {
                for j in (i + 1)..parsed.len() {
                    if parsed[i].field != parsed[j].field {
                        continue;
                    }
                    if let Some(reason) = Self::contradiction_reason(
                        (&parsed[i].op, &parsed[i].field, parsed[i].value),
                        (&parsed[j].op, &parsed[j].field, parsed[j].value),
                    ) {
                        let is_principal = contradictions.is_empty();
                        if is_principal {
                            principal = Some(reason.clone());
                            focus = parsed[i].field.clone();
                        }
                        contradictions.push(json!({
                            "description": reason,
                            "is_principal": is_principal,
                            "affected_step": parsed[i].field,
                            "severity": 3,
                            "resolution": "relax one of the conflicting bounds",
                        }));
                        allocation.insert(parsed[i].field.clone(), json!(0.5));
                    }
                }
            }
        } else if let Some(task) = input.get("task").and_then(|v| v.as_str()) {
            for (description, affected_step) in Self::text_contradictions(task) {
                let is_principal = contradictions.is_empty();
                if is_principal {
                    principal = Some(description.clone());
                    focus = affected_step.clone();
                }
                contradictions.push(json!({
                    "description": description,
                    "is_principal": is_principal,
                    "affected_step": affected_step,
                    "severity": 1,
                    "resolution": "align conflicting instructions",
                }));
                allocation.insert("text".to_string(), json!(0.5));
            }
        }

        let principal_contradiction = principal.unwrap_or_else(|| "none".to_string());
        Ok(json!({
            "contradictions": contradictions,
            "principal_contradiction": principal_contradiction,
            "recommended_focus": focus,
            "resource_allocation": Value::Object(allocation),
        }))
    }
}

// ── prompt_audit：提示词审计（对齐原版 builtins/prompt_audit.rs，AI 判断降级为 8-step 规则） ──

/// 8-step 框架的规则检测（原版由 AI 判断，Round-1 降级为纯字符串规则）。
/// 返回 (present, suggestion)。suggestion 文案对齐原版 AUDIT_SYSTEM 模板。
fn audit_rule(prompt: &str) -> Vec<(&'static str, bool, &'static str)> {
    let lower = prompt.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    vec![
        (
            "role_assignment",
            has(&["you are", "你是", "你是一个", "你扮演", "act as", "扮演"]),
            "Start with 'You are a...' or an equivalent role identity",
        ),
        (
            "task_context",
            prompt.trim().chars().count() >= 30 && has(&["task", "目标", "需要", "做", "write", "create", "构建", "分析"]),
            "Explain what to do and why (task context)",
        ),
        (
            "rules",
            has(&["must", "don't", "do not", "avoid", "never", "不得", "禁止", "必须", "不要", "不能"]),
            "Add detailed rules — boundaries, constraints, do/don't",
        ),
        (
            "examples",
            has(&["<example", "例如", "for example", "示例", "e.g.", "比如"]),
            "Add 1-2 few-shot examples wrapped in <example> tags",
        ),
        (
            "xml_input_tags",
            has(&["<input>", "<data>", "<text>", "<prompt", "<user", "<tag>", "<xml", "<goal", "<context"]),
            "Wrap variable input in XML tags like <tag>...</tag>",
        ),
        (
            "output_format",
            has(&["output", "format", "json", "返回", "输出", "格式", "respond"]),
            "Add a specific output format instruction near the bottom",
        ),
        (
            "chain_of_thought",
            has(&["step by step", "chain of thought", "一步一步", "逐步分析", "think through"]),
            "Add 'first analyze step by step' for complex reasoning",
        ),
        (
            "anti_hallucination",
            has(&["if unsure", "say unknown", "if not sure", "不确定", "不知道就说", "承认不知道", "say i don't know"]),
            "Add an out: 'if unsure, say unknown'",
        ),
    ]
}

pub struct PromptAuditTool {
    descriptor: ToolDescriptor,
}

impl PromptAuditTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "prompt_audit".into(),
                description: "按 8-step 框架审计提示词质量（Role/Task/Rules/Examples/XML/Output/CoT/Anti-hallucination）。输入 {prompt, model?}，返回 {audit:{score,framework,missing_items,critical_issues,improved_prompt,summary}, target_model, prompt_chars, adaptation_hint}。AI 判断已降级为纯规则。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string", "description": "待审计的提示词文本" },
                        "model": { "type": "string", "description": "目标模型（claude/gemini/gpt/openai/deepseek）" }
                    },
                    "required": ["prompt"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }

    /// 对齐原版 adaptation_hint：按目标模型给出适配提示。
    fn adaptation_hint(model: &str) -> &'static str {
        match model {
            "gemini" => "Gemini requires Persona/Task/Context/Format four elements. Convert XML tags to this structure.",
            "gpt" | "openai" => "GPT-5.5 prefers outcome-first over step-by-step. Consider shortening.",
            "deepseek" => "DeepSeek uses CO-STAR framework. Add Context/Objective/Style/Tone/Audience/Response.",
            _ => "Claude-native XML format should work as-is.",
        }
    }
}

impl Default for PromptAuditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PromptAuditTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("prompt is required"))?;
        let model = input
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("claude");

        // 空 prompt：对齐原版空分支语义。
        if prompt.trim().is_empty() {
            let audit = json!({
                "score": 0.0,
                "summary": "empty prompt — nothing to audit",
                "framework": {},
                "missing_items": ["prompt"],
                "critical_issues": ["No prompt provided"]
            });
            return Ok(json!({
                "audit": audit,
                "target_model": model,
                "prompt_chars": 0,
                "adaptation_hint": Self::adaptation_hint(model),
            }));
        }

        // 8-step 规则检测（原版为 AI 判断，Round-1 降级为纯规则）。
        let checks = audit_rule(prompt);
        let mut framework = serde_json::Map::new();
        let mut missing_items = Vec::new();
        let mut present = 0usize;
        for (key, ok, suggestion) in checks {
            let entry = if ok {
                present += 1;
                json!({"present": true, "suggestion": ""})
            } else {
                missing_items.push(key.to_string());
                json!({"present": false, "suggestion": suggestion})
            };
            framework.insert(key.to_string(), entry);
        }

        let total = 8usize;
        let missing = total - present;
        let audit = json!({
            "score": present as f64 / total as f64,
            "framework": Value::Object(framework),
            "missing_items": missing_items,
            "critical_issues": [],
            "improved_prompt": "",
            "summary": format!("{} of {} framework items missing. Score: {}/{}", missing, total, present, total),
        });

        Ok(json!({
            "audit": audit,
            "target_model": model,
            "prompt_chars": prompt.chars().count(),
            "adaptation_hint": Self::adaptation_hint(model),
        }))
    }
}

// ── strategic_plan：战略规划（对齐原版 SYSTEM_STRATEGIC_PLAN 字段） ──

pub struct StrategicPlanTool {
    descriptor: ToolDescriptor,
}

impl StrategicPlanTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "strategic_plan".into(),
                description: "原型战框架战略规划：信息稀少→防御、适中→相持、清晰→进攻。输入 {objectives, resources} 或 {task}，返回 {current_phase, phase_rationale, estimated_complexity, steps}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string" },
                        "objectives": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "priority": { "type": "integer", "minimum": 1, "maximum": 10 },
                                    "resource_type": { "type": "string" },
                                    "required_capacity": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["id"]
                            }
                        },
                        "resources": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "type": { "type": "string" },
                                    "capacity": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["id"]
                            }
                        }
                    }
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

impl Default for StrategicPlanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for StrategicPlanTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let objectives = input.get("objectives").and_then(|v| v.as_array());
        let resources = input.get("resources").and_then(|v| v.as_array());

        if let (Some(objectives), Some(resources)) = (objectives, resources) {
            // 资源剩余容量记账。
            let mut remaining: Vec<u64> = resources
                .iter()
                .map(|r| r.get("capacity").and_then(|v| v.as_u64()).unwrap_or(1).max(1))
                .collect();

            // 按 priority 升序（缺省 5），稳定排序。
            let mut indexed: Vec<(usize, i64)> = objectives
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let p = o.get("priority").and_then(|v| v.as_i64()).unwrap_or(5);
                    (i, p.clamp(1, 10))
                })
                .collect();
            indexed.sort_by_key(|&(_, p)| p);

            let total_capacity: u64 = remaining.iter().sum::<u64>().max(1);
            let mut steps = Vec::new();
            let mut covered = 0usize;

            for (oi, _p) in &indexed {
                let obj = &objectives[*oi];
                let oid = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("objective");
                let want_type = obj
                    .get("resource_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let need = obj
                    .get("required_capacity")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
                    .max(1);

                let mut assigned = false;
                for (ri, r) in resources.iter().enumerate() {
                    if remaining[ri] < need {
                        continue;
                    }
                    let rtype = r.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if !want_type.is_empty() && rtype != want_type {
                        continue;
                    }
                    remaining[ri] -= need;
                    assigned = true;
                    let weight = need as f64 / total_capacity as f64;
                    steps.push(json!({
                        "name": oid,
                        "phase": "offense",
                        "action": "execute",
                        "capability": want_type,
                        "resource_weight": (weight * 100.0).round() / 100.0,
                    }));
                    break;
                }
                if assigned {
                    covered += 1;
                }
            }

            let total = objectives.len().max(1);
            let rate = covered as f64 / total as f64;
            let current_phase = if rate >= 0.8 {
                "offense"
            } else if rate >= 0.4 {
                "stalemate"
            } else {
                "defense"
            };
            let phase_rationale = format!(
                "resource coverage {covered}/{total} ({:.0}%) — {}",
                rate * 100.0,
                current_phase
            );
            let estimated_complexity = if objectives.len() > 5 {
                "high"
            } else if objectives.len() > 2 {
                "medium"
            } else {
                "low"
            };

            // 未覆盖目标补为 defense/stalemate 阶段步骤。
            let mut covered_flags = vec![false; objectives.len()];
            for step in &steps {
                if let Some(name) = step["name"].as_str() {
                    for (oi, _p) in &indexed {
                        if objectives[*oi].get("id").and_then(|v| v.as_str()) == Some(name) {
                            covered_flags[*oi] = true;
                        }
                    }
                }
            }
            for (oi, _p) in &indexed {
                let obj = &objectives[*oi];
                let oid = obj.get("id").and_then(|v| v.as_str()).unwrap_or("objective");
                if !covered_flags[*oi] {
                    steps.push(json!({
                        "name": oid,
                        "phase": if rate >= 0.4 { "stalemate" } else { "defense" },
                        "action": "gather resources",
                        "capability": "",
                        "resource_weight": 0.0,
                    }));
                }
            }

            return Ok(json!({
                "current_phase": current_phase,
                "phase_rationale": phase_rationale,
                "estimated_complexity": estimated_complexity,
                "steps": steps,
            }));
        }

        // 纯 task 文本降级：按信息量判断阶段。
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let trimmed = task.trim();
        let char_len = trimmed.chars().count();
        let current_phase = if char_len >= 120 {
            "offense"
        } else if char_len >= 40 {
            "stalemate"
        } else {
            "defense"
        };
        let phase_rationale = format!(
            "information available: {} chars — phase {current_phase}",
            char_len
        );
        let estimated_complexity = if char_len >= 200 { "high" } else if char_len >= 80 { "medium" } else { "low" };
        let steps = vec![json!({
            "name": "step1",
            "phase": current_phase,
            "action": "execute",
            "capability": "",
            "resource_weight": 1.0,
        })];

        Ok(json!({
            "current_phase": current_phase,
            "phase_rationale": phase_rationale,
            "estimated_complexity": estimated_complexity,
            "steps": steps,
        }))
    }
}

// ── task_dialectic：三阶辩证法（对齐原版 SYSTEM_DIALECTIC 字段） ──

pub struct TaskDialecticTool {
    descriptor: ToolDescriptor,
}

impl TaskDialecticTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "task_dialectic".into(),
                description: "对任务运行正题→反题→合题三阶辩证。输入 {task}，返回 {thesis, antithesis, synthesis}（各含 content/strengths/weaknesses/confidence）（原版字段）。任务不明确时全部 confidence 置 0。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string" }
                    },
                    "required": ["task"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }

    /// 规则版明确性：足够长且含目标动词。
    fn is_clear(task: &str) -> bool {
        let len = task.trim().chars().count();
        if len < 15 {
            return false;
        }
        let lower = task.to_lowercase();
        lower.contains("实现") || lower.contains("分析") || lower.contains("写") || lower.contains("构建")
            || lower.contains("optimize") || lower.contains("create") || lower.contains("build")
            || lower.contains("write") || lower.contains("analy") || lower.contains("refactor")
            || lower.contains("设计") || lower.contains("测试") || lower.contains("修复")
    }
}

impl Default for TaskDialecticTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskDialecticTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("task is required"))?;
        let trimmed = task.trim();

        // 任务不清 → 各 confidence 置 0，不臆造方案（对齐原版规则）。
        if trimmed.is_empty() || !Self::is_clear(trimmed) {
            let base = json!({"content": "", "strengths": [], "weaknesses": ["task too vague to analyze"], "confidence": 0.0});
            return Ok(json!({
                "thesis": base,
                "antithesis": base,
                "synthesis": base,
            }));
        }

        // 规则降级：以明确性生成三命题。
        let thesis_content = trimmed.to_string();
        let antithesis_content = format!("Critique assumptions and propose alternative view of: {}", trimmed);
        let synthesis_content = format!("Synthesize thesis and antithesis into refined approach for: {}", trimmed);

        Ok(json!({
            "thesis": {
                "content": thesis_content,
                "strengths": ["task is explicitly stated", "actionable direction present"],
                "weaknesses": [],
                "confidence": 0.8,
            },
            "antithesis": {
                "content": antithesis_content,
                "strengths": ["challenges initial framing", "considers alternative assumptions"],
                "weaknesses": ["may over-critique without evidence"],
                "confidence": 0.5,
            },
            "synthesis": {
                "content": synthesis_content,
                "strengths": ["combines best of thesis and antithesis"],
                "weaknesses": [],
                "confidence": 0.7,
            },
        }))
    }
}

// ── dialectical_retry：根因分析与辩证重试（对齐原版 SYSTEM_RETRY 字段） ──

pub struct DialecticalRetryTool {
    descriptor: ToolDescriptor,
}

impl DialecticalRetryTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "dialectical_retry".into(),
                description: "分析失败根因并给出替代策略。输入 {task, error, strategy?}，返回 {root_cause, lesson, next_strategy}（原版字段）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "task": { "type": "string" },
                        "error": { "type": "string", "description": "失败错误信息" },
                        "strategy": { "type": "string", "description": "已使用的策略（可选）" }
                    },
                    "required": ["error"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }

    /// 规则版根因分类：从 error 文本关键词判定。
    fn classify_root_cause(error: &str) -> &'static str {
        let lower = error.to_lowercase();
        if lower.contains("timeout") || lower.contains("超时") || lower.contains("timed out") {
            "timeout"
        } else if lower.contains("not found") || lower.contains("404") || lower.contains("不存在") || lower.contains("no such") {
            "resource not found"
        } else if lower.contains("permission") || lower.contains("denied") || lower.contains("403") || lower.contains("权限") || lower.contains("forbidden") {
            "permission denied"
        } else if lower.contains("parse") || lower.contains("invalid") || lower.contains("格式") || lower.contains("malformed") || lower.contains("syntax") {
            "malformed input"
        } else if lower.contains("connect") || lower.contains("network") || lower.contains("连接") || lower.contains("unreachable") {
            "network failure"
        } else {
            "unknown"
        }
    }

    fn next_strategy_for(root_cause: &str) -> &'static str {
        match root_cause {
            "timeout" => "retry with longer timeout and exponential backoff",
            "resource not found" => "verify resource existence before retry",
            "permission denied" => "request required permission or narrow to allowed scope",
            "malformed input" => "re-validate input format before retry",
            "network failure" => "check connectivity and retry with retry-after backoff",
            _ => "inspect logs and reproduce manually before retrying",
        }
    }
}

impl Default for DialecticalRetryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DialecticalRetryTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: Value) -> ForgeResult<Value> {
        let error = input
            .get("error")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("error is required"))?;
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let root_cause = if error.trim().is_empty() {
            "unknown"
        } else {
            Self::classify_root_cause(error)
        };
        let lesson = if task.trim().is_empty() {
            format!("retry after addressing: {root_cause}")
        } else {
            format!("'{}' failed due to {root_cause}; do not repeat same strategy blindly", truncate_mid(task.trim(), 60))
        };
        let next_strategy = Self::next_strategy_for(root_cause);

        Ok(json!({
            "root_cause": root_cause,
            "lesson": lesson,
            "next_strategy": next_strategy,
        }))
    }
}

/// 注册全部 zl 工具到 router（9 工具，全部 ReadOnly）。
/// 清单按原版 aion-router builtins：zl.rs 8 工具 + prompt_audit.rs 1 工具。
pub fn register_all(router: &forge_exec::ToolRouter) -> ForgeResult<()> {
    router.register(Box::new(StrategicPlanTool::new()))?;
    router.register(Box::new(TaskDialecticTool::new()))?;
    router.register(Box::new(ContradictionAnalyzeTool::new()))?;
    router.register(Box::new(CompileContractTool::new()))?;
    router.register(Box::new(CheckSufficiencyTool::new()))?;
    router.register(Box::new(VerifyResultTool::new()))?;
    router.register(Box::new(DetectDriftTool::new()))?;
    router.register(Box::new(DialecticalRetryTool::new()))?;
    router.register(Box::new(PromptAuditTool::new()))?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    // ── check_sufficiency（原版字段：sufficient/confidence/missing/recommendation） ──
    #[tokio::test]
    async fn test_check_sufficiency_ok() {
        let tool = CheckSufficiencyTool::new();
        let result = tool
            .invoke(json!({
                "requirements": [{"id": "R1", "resource_type": "ecs", "amount": 2}],
                "resources": [
                    {"id": "ecs-a", "resource_type": "ecs", "status": "available"},
                    {"id": "ecs-b", "resource_type": "ecs"}
                ]
            }))
            .await
            .unwrap();
        assert_eq!(result["sufficient"], true);
        assert_eq!(result["confidence"], 1.0);
        assert_eq!(result["missing"].as_array().unwrap().len(), 0);
        assert_eq!(result["recommendation"], "proceed");
    }

    #[tokio::test]
    async fn test_check_sufficiency_partial() {
        let tool = CheckSufficiencyTool::new();
        let result = tool
            .invoke(json!({
                "requirements": [
                    {"id": "R1", "resource_type": "ecs"},
                    {"id": "R2", "resource_type": "obs"}
                ],
                "resources": [{"id": "ecs-a", "resource_type": "ecs"}]
            }))
            .await
            .unwrap();
        assert_eq!(result["sufficient"], false);
        assert_eq!(result["confidence"], 0.5);
        assert_eq!(result["missing"].as_array().unwrap().len(), 1);
        assert_eq!(result["recommendation"], "gather_more");
    }

    #[tokio::test]
    async fn test_check_sufficiency_no_match_clarify() {
        let tool = CheckSufficiencyTool::new();
        let result = tool
            .invoke(json!({
                "requirements": [{"id": "R1", "resource_type": "ecs"}],
                "resources": [{"id": "ecs-x", "resource_type": "ecs", "status": "failed"}]
            }))
            .await
            .unwrap();
        assert_eq!(result["sufficient"], false);
        assert_eq!(result["recommendation"], "clarify_with_user");
    }

    #[tokio::test]
    async fn test_check_sufficiency_task_text_degrade() {
        let tool = CheckSufficiencyTool::new();
        let result = tool
            .invoke(json!({"task": "构建一个高可用系统需要多少资源？"}))
            .await
            .unwrap();
        assert_eq!(result["sufficient"], false);
        assert_eq!(result["confidence"], 0.3);
        assert_eq!(result["recommendation"], "gather_more");
    }

    // ── verify_result（原版字段：passed/score/criteria_results/verdict/feedback） ──
    #[tokio::test]
    async fn test_verify_result_accept() {
        let tool = VerifyResultTool::new();
        let result = tool
            .invoke(json!({
                "expected": {"a": 1, "b": {"c": [1, 2]}},
                "actual": {"a": 1, "b": {"c": [1, 2]}}
            }))
            .await
            .unwrap();
        assert_eq!(result["passed"], true);
        assert_eq!(result["score"], 1.0);
        assert_eq!(result["verdict"], "accept");
        let cr = result["criteria_results"].as_array().unwrap();
        assert_eq!(cr.len(), 3); // a, b.c[0], b.c[1]
        assert!(cr.iter().all(|c| c["met"] == true));
    }

    #[tokio::test]
    async fn test_verify_result_retry_and_escalate() {
        let tool = VerifyResultTool::new();
        // 部分命中 → retry
        let result = tool
            .invoke(json!({
                "expected": {"a": 1, "b": 2},
                "actual": {"a": 1, "b": 9}
            }))
            .await
            .unwrap();
        assert_eq!(result["passed"], false);
        assert_eq!(result["score"], 0.5);
        assert_eq!(result["verdict"], "retry");
        assert!(result["feedback"].as_str().unwrap().contains("criteria met"));
        // 全不中 → escalate
        let result = tool
            .invoke(json!({
                "expected": {"a": 1, "b": 2, "c": 3},
                "actual": {"a": 9, "b": 9, "c": 9}
            }))
            .await
            .unwrap();
        assert_eq!(result["score"], 0.0);
        assert_eq!(result["verdict"], "escalate");
    }

    // ── compile_contract（原版字段：task_summary/acceptance_criteria/...） ──
    #[tokio::test]
    async fn test_compile_contract_structure() {
        let tool = CompileContractTool::new();
        let result = tool
            .invoke(json!({"task": "实现一个 Rust 函数计算斐波那契数列。它必须处理负数输入。完成后运行测试验证。"}))
            .await
            .unwrap();
        assert_eq!(result["ok"], Value::Null); // 原版无 ok 字段
        assert!(result["task_summary"].as_str().unwrap().contains("斐波那契"));
        assert!(!result["acceptance_criteria"].as_array().unwrap().is_empty());
        assert_eq!(result["expected_outputs"][0]["type"], "code");
        assert!(!result["required_context"].as_array().unwrap().is_empty());
        assert_eq!(result["complexity"], "low");
        assert!(result["estimated_steps"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn test_compile_contract_explicit_criteria() {
        let tool = CompileContractTool::new();
        let result = tool
            .invoke(json!({
                "task": "写一个 API 网关配置",
                "criteria": ["路由正确", "限流生效", "日志可查"]
            }))
            .await
            .unwrap();
        let criteria = result["acceptance_criteria"].as_array().unwrap();
        assert_eq!(criteria.len(), 3);
        assert_eq!(criteria[0], "路由正确");
        assert_eq!(result["estimated_steps"], 3);
    }

    #[tokio::test]
    async fn test_compile_contract_empty_task_error() {
        let tool = CompileContractTool::new();
        let result = tool.invoke(json!({"task": "   "})).await;
        assert!(result.is_err(), "empty task should error");
    }

    // ── detect_drift（原版字段：on_track/drift_score/drift_description/correction） ──
    #[tokio::test]
    async fn test_detect_drift_clean() {
        let tool = DetectDriftTool::new();
        let result = tool
            .invoke(json!({
                "expected": {"spec": {"replicas": 3}},
                "actual": {"spec": {"replicas": 3}}
            }))
            .await
            .unwrap();
        assert_eq!(result["on_track"], true);
        assert_eq!(result["drift_score"], 0.0);
        assert_eq!(result["correction"], "");
    }

    #[tokio::test]
    async fn test_detect_drift_changed() {
        let tool = DetectDriftTool::new();
        let result = tool
            .invoke(json!({
                "expected": {"spec": {"replicas": 3, "tag": "v1"}},
                "actual": {"spec": {"replicas": 1, "extra": true}}
            }))
            .await
            .unwrap();
        assert_eq!(result["on_track"], false);
        assert!(result["drift_score"].as_f64().unwrap() > 0.0);
        assert!(result["drift_description"].as_str().unwrap().contains("drifted"));
        assert!(result["correction"].as_str().unwrap().contains("restore"));
    }

    // ── contradiction_analyze（原版字段：contradictions/principal_contradiction/...） ──
    #[tokio::test]
    async fn test_contradiction_analyze_conflict() {
        let tool = ContradictionAnalyzeTool::new();
        let result = tool
            .invoke(json!({
                "constraints": [
                    {"id": "C1", "field": "cpu", "op": "gte", "value": 8},
                    {"id": "C2", "field": "cpu", "op": "lte", "value": 4},
                    {"id": "C3", "field": "mem", "op": "gte", "value": 16}
                ]
            }))
            .await
            .unwrap();
        let contradictions = result["contradictions"].as_array().unwrap();
        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0]["is_principal"], true);
        assert_eq!(contradictions[0]["affected_step"], "cpu");
        assert_ne!(result["principal_contradiction"], "none");
        assert_eq!(result["recommended_focus"], "cpu");
        assert!(result["resource_allocation"]["cpu"].is_f64());
    }

    #[tokio::test]
    async fn test_contradiction_analyze_clean() {
        let tool = ContradictionAnalyzeTool::new();
        let result = tool
            .invoke(json!({
                "constraints": [
                    {"id": "C1", "field": "cpu", "op": "gte", "value": 4},
                    {"id": "C2", "field": "cpu", "op": "lte", "value": 8}
                ]
            }))
            .await
            .unwrap();
        assert_eq!(result["contradictions"].as_array().unwrap().len(), 0);
        assert_eq!(result["principal_contradiction"], "none");
    }

    #[tokio::test]
    async fn test_contradiction_analyze_text() {
        let tool = ContradictionAnalyzeTool::new();
        let result = tool
            .invoke(json!({"task": "必须尽快交付，但禁止加班。"}))
            .await
            .unwrap();
        let contradictions = result["contradictions"].as_array().unwrap();
        assert!(!contradictions.is_empty(), "text opposition should be detected");
    }

    // ── prompt_audit（原版 8-step） ──
    #[tokio::test]
    async fn test_prompt_audit_full_framework() {
        let tool = PromptAuditTool::new();
        let result = tool
            .invoke(json!({
                "prompt": "You are a code reviewer. Your task is to analyze the given code and explain why it fails. Rules: you must not modify code, avoid guessing. Example: <example>in: x, out: y</example> Input data: <data>...</data> Output format: return JSON. First analyze step by step. If unsure, say unknown.",
                "model": "claude"
            }))
            .await
            .unwrap();
        let audit = &result["audit"];
        assert_eq!(audit["score"], 1.0);
        assert_eq!(audit["missing_items"].as_array().unwrap().len(), 0);
        assert_eq!(result["target_model"], "claude");
        assert!(result["adaptation_hint"].as_str().unwrap().contains("Claude"));
    }

    #[tokio::test]
    async fn test_prompt_audit_partial() {
        let tool = PromptAuditTool::new();
        let result = tool
            .invoke(json!({"prompt": "Write a function in Python."}))
            .await
            .unwrap();
        let audit = &result["audit"];
        assert!(audit["score"].as_f64().unwrap() < 1.0);
        assert!(!audit["missing_items"].as_array().unwrap().is_empty());
        assert_eq!(audit["framework"].as_object().unwrap().len(), 8);
    }

    #[tokio::test]
    async fn test_prompt_audit_empty() {
        let tool = PromptAuditTool::new();
        let result = tool
            .invoke(json!({"prompt": "   "}))
            .await
            .unwrap();
        assert_eq!(result["audit"]["score"], 0.0);
        assert_eq!(result["audit"]["critical_issues"][0], "No prompt provided");
    }

    #[tokio::test]
    async fn test_prompt_audit_adaptation_hint() {
        let tool = PromptAuditTool::new();
        let result = tool
            .invoke(json!({"prompt": "You are a helper. 请分析。Rules: must not. Output format: JSON.", "model": "deepseek"}))
            .await
            .unwrap();
        assert!(result["adaptation_hint"].as_str().unwrap().contains("DeepSeek"));
    }

    // ── strategic_plan（原版字段：current_phase/phase_rationale/...） ──
    #[tokio::test]
    async fn test_strategic_plan_full_coverage_offense() {
        let tool = StrategicPlanTool::new();
        let result = tool
            .invoke(json!({
                "objectives": [
                    {"id": "O1", "priority": 1, "resource_type": "engineer", "required_capacity": 2},
                    {"id": "O2", "priority": 5, "resource_type": "engineer", "required_capacity": 1}
                ],
                "resources": [{"id": "dev-a", "type": "engineer", "capacity": 4}]
            }))
            .await
            .unwrap();
        assert_eq!(result["current_phase"], "offense");
        assert!(result["phase_rationale"].as_str().unwrap().contains("100%"));
        assert_eq!(result["estimated_complexity"], "low");
        let steps = result["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["name"], "O1");
        assert!(steps[0]["resource_weight"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_strategic_plan_partial_stalemate() {
        let tool = StrategicPlanTool::new();
        let result = tool
            .invoke(json!({
                "objectives": [
                    {"id": "O1", "priority": 1, "resource_type": "engineer", "required_capacity": 1},
                    {"id": "O2", "priority": 2, "resource_type": "designer", "required_capacity": 1}
                ],
                "resources": [{"id": "dev-a", "type": "engineer", "capacity": 1}]
            }))
            .await
            .unwrap();
        assert_eq!(result["current_phase"], "stalemate");
        let steps = result["steps"].as_array().unwrap();
        // O1 covered(offense) + O2 未覆盖补 step
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["name"], "O1");
        assert_eq!(steps[1]["name"], "O2");
        assert_eq!(steps[1]["phase"], "stalemate");
    }

    #[tokio::test]
    async fn test_strategic_plan_task_text() {
        let tool = StrategicPlanTool::new();
        let result = tool
            .invoke(json!({"task": "信息极稀少，只能保守推进，先做侦察。"}))
            .await
            .unwrap();
        // 短文本 → defense
        assert_eq!(result["current_phase"], "defense");
    }

    // ── task_dialectic（原版字段：thesis/antithesis/synthesis） ──
    #[tokio::test]
    async fn test_task_dialectic_clear_task() {
        let tool = TaskDialecticTool::new();
        let result = tool
            .invoke(json!({"task": "实现一个带超时重试的 HTTP 客户端，要求并发安全并记录日志。"}))
            .await
            .unwrap();
        for side in ["thesis", "antithesis", "synthesis"] {
            let s = &result[side];
            assert!(!s["content"].as_str().unwrap().is_empty());
            assert!(s["confidence"].as_f64().unwrap() >= 0.0);
        }
        assert!(result["thesis"]["confidence"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_task_dialectic_vague_task_zero_confidence() {
        let tool = TaskDialecticTool::new();
        let result = tool
            .invoke(json!({"task": "随便"})).await
            .unwrap();
        assert_eq!(result["thesis"]["confidence"], 0.0);
        assert_eq!(result["antithesis"]["confidence"], 0.0);
        assert_eq!(result["synthesis"]["confidence"], 0.0);
    }

    // ── dialectical_retry（原版字段：root_cause/lesson/next_strategy） ──
    #[tokio::test]
    async fn test_dialectical_retry_timeout() {
        let tool = DialecticalRetryTool::new();
        let result = tool
            .invoke(json!({"task": "调用外部 API", "error": "request timed out after 30s"}))
            .await
            .unwrap();
        assert_eq!(result["root_cause"], "timeout");
        assert!(result["next_strategy"].as_str().unwrap().contains("timeout"));
    }

    #[tokio::test]
    async fn test_dialectical_retry_not_found() {
        let tool = DialecticalRetryTool::new();
        let result = tool
            .invoke(json!({"task": "读取配置文件", "error": "file not found: config.toml"}))
            .await
            .unwrap();
        assert_eq!(result["root_cause"], "resource not found");
    }

    #[tokio::test]
    async fn test_dialectical_retry_unknown() {
        let tool = DialecticalRetryTool::new();
        let result = tool
            .invoke(json!({"task": "x", "error": "weird failure"}))
            .await
            .unwrap();
        assert_eq!(result["root_cause"], "unknown");
    }

    // ── register_all ──
    #[tokio::test]
    async fn test_register_all_registers_nine() {
        let router = forge_exec::ToolRouter::new();
        register_all(&router).unwrap();
        let list = router.list();
        assert_eq!(list.len(), 9);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        for expected in [
            "strategic_plan",
            "task_dialectic",
            "contradiction_analyze",
            "compile_contract",
            "check_sufficiency",
            "verify_result",
            "detect_drift",
            "dialectical_retry",
            "prompt_audit",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        // 全部 ReadOnly
        for d in &list {
            assert_eq!(d.permission, PermissionLevel::ReadOnly);
        }
    }
}
