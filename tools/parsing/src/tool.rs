//! 解析工具：yaml_parse / json_parse / toml_parse / csv_parse / pdf_parse
//!
//! 5 个纯计算工具（档 A，#W-14 批次1）。
//! 封装模式：单字段 `descriptor: ToolDescriptor` struct + new() 预构造
//! （仿 worklog / fs / search 真实模式，async_trait + #[tokio::test]）。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::json;

fn err(msg: impl Into<String>) -> ForgeError {
    ForgeError::InvalidState(msg.into())
}

// ── yaml_parse ──

pub struct YamlParseTool {
    descriptor: ToolDescriptor,
}

impl YamlParseTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "yaml_parse".into(),
                description: "解析 YAML 文本为 JSON 对象。输入 {text}，返回 {ok, json, keys, top_level}。"
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "YAML 文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for YamlParseTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        use yaml_rust2::YamlLoader;
        let docs = YamlLoader::load_from_str(text).map_err(|e| err(format!("YAML 解析失败: {e}")))?;
        let json_vals: Vec<serde_json::Value> = docs.iter().map(yaml_to_json).collect();
        let first = json_vals.first().unwrap_or(&serde_json::Value::Null);
        let keys = match first {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            serde_json::Value::Array(items) if !items.is_empty() => match &items[0] {
                serde_json::Value::Object(map) => map.keys().cloned().collect(),
                _ => vec![],
            },
            _ => vec![],
        };
        let top_level = match first {
            serde_json::Value::Object(map) => format!("mapping({})", map.len()),
            serde_json::Value::Array(items) => format!("sequence({})", items.len()),
            serde_json::Value::Null => "null".to_string(),
            _ => "scalar".to_string(),
        };
        Ok(json!({
            "ok": true,
            "json": json_vals,
            "keys": keys,
            "top_level": top_level,
        }))
    }
}

fn yaml_to_json(y: &yaml_rust2::Yaml) -> serde_json::Value {
    use yaml_rust2::Yaml as Y;
    match y {
        Y::Null | Y::BadValue => serde_json::Value::Null,
        Y::Boolean(b) => serde_json::Value::Bool(*b),
        Y::Integer(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
        Y::Real(s) => match s.parse::<f64>() {
            Ok(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::String(s.clone())),
            Err(_) => serde_json::Value::String(s.clone()),
        },
        Y::String(s) => serde_json::Value::String(s.clone()),
        Y::Array(items) => {
            let arr: Vec<serde_json::Value> = items.iter().map(yaml_to_json).collect();
            serde_json::Value::Array(arr)
        }
        Y::Hash(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    Y::String(s) => s.clone(),
                    Y::Integer(i) => i.to_string(),
                    Y::Real(s) => s.clone(),
                    Y::Boolean(b) => b.to_string(),
                    Y::Null => "null".to_string(),
                    other => yaml_to_json(other).to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(out)
        }
        Y::Alias(_) => serde_json::Value::Null,
    }
}

// ── json_parse ──

pub struct JsonParseTool {
    descriptor: ToolDescriptor,
}

impl JsonParseTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "json_parse".into(),
                description: "解析 JSON 文本为结构化 Value。输入 {text}，返回 {ok, json, keys}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "JSON 文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for JsonParseTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(text).map_err(|e| err(format!("JSON 解析失败: {e}")))?;
        let keys = match &parsed {
            serde_json::Value::Object(map) => map.keys().cloned().collect::<Vec<_>>(),
            _ => vec![],
        };
        Ok(json!({ "ok": true, "json": parsed, "keys": keys }))
    }
}

// ── toml_parse ──

pub struct TomlParseTool {
    descriptor: ToolDescriptor,
}

impl TomlParseTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "toml_parse".into(),
                description: "解析 TOML 文本为 JSON 对象。输入 {text}，返回 {ok, json, sections}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "TOML 文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TomlParseTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        let table: toml::Table =
            toml::from_str(text).map_err(|e| err(format!("TOML 解析失败: {e}")))?;
        let json_val: serde_json::Value = serde_json::to_value(table)
            .map_err(|e| ForgeError::Config(format!("TOML 转 JSON 失败: {e}")))?;
        let sections = match &json_val {
            serde_json::Value::Object(map) => map.keys().cloned().collect::<Vec<_>>(),
            _ => vec![],
        };
        Ok(json!({ "ok": true, "json": json_val, "sections": sections }))
    }
}

// ── csv_parse ──

pub struct CsvParseTool {
    descriptor: ToolDescriptor,
}

impl CsvParseTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "csv_parse".into(),
                description: "解析 CSV 文本为 JSON 行数组。输入 {text}，返回 {ok, headers, row_count, rows}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "CSV 文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for CsvParseTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        let mut reader = csv::ReaderBuilder::new().has_headers(true).from_reader(text.as_bytes());
        let headers = reader
            .headers()
            .map_err(|e| err(format!("CSV 头解析失败: {e}")))?
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut rows: Vec<serde_json::Value> = Vec::new();
        for row in reader.records() {
            let rec = row.map_err(|e| err(format!("CSV 行解析失败: {e}")))?;
            let mut obj = serde_json::Map::new();
            for (i, h) in headers.iter().enumerate() {
                let val = rec.get(i).unwrap_or("").to_string();
                obj.insert(h.clone(), serde_json::Value::String(val));
            }
            rows.push(serde_json::Value::Object(obj));
        }
        Ok(json!({
            "ok": true,
            "headers": headers,
            "row_count": rows.len(),
            "rows": rows,
        }))
    }
}

// ── pdf_parse（round1 占位）──

pub struct PdfParseTool {
    descriptor: ToolDescriptor,
}

impl PdfParseTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "pdf_parse".into(),
                description: "解析 PDF 文件（round1 占位，待接 pdf-extract 引擎后真解析）。输入 {file_path}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "PDF 文件路径" }
                    },
                    "required": ["file_path"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for PdfParseTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("file_path is required"))?;
        // round1 占位：返回 pending_engine 降级，不崩溃不静默
        let _ = file_path;
        Ok(json!({
            "status": "pending_engine",
            "message": "PDF engine (pdf-extract) channel not wired in round 1; tool is a mechanical shell",
            "tool": "pdf_parse"
        }))
    }
}

/// 注册所有解析工具到 router。
pub fn register_all(router: &forge_exec::ToolRouter) -> ForgeResult<()> {
    router.register(Box::new(YamlParseTool::new()))?;
    router.register(Box::new(JsonParseTool::new()))?;
    router.register(Box::new(TomlParseTool::new()))?;
    router.register(Box::new(CsvParseTool::new()))?;
    router.register(Box::new(PdfParseTool::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_yaml_parse_roundtrip() {
        let tool = YamlParseTool::new();
        let result = tool
            .invoke(json!({"text": "a: 1\nb: two\nc:\n  - x\n  - y"}))
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["json"][0]["a"], 1);
        assert_eq!(result["json"][0]["b"], "two");
        assert_eq!(result["json"][0]["c"][0], "x");
        assert_eq!(result["top_level"], "mapping(3)");
    }

    #[tokio::test]
    async fn test_yaml_parse_missing_text() {
        let tool = YamlParseTool::new();
        let result = tool.invoke(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_json_parse_roundtrip() {
        let tool = JsonParseTool::new();
        let result = tool
            .invoke(json!({"text": "{\"x\":1,\"y\":true}"}))
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["json"]["x"], 1);
        assert_eq!(result["json"]["y"], true);
        let keys: Vec<String> = result["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(keys, vec!["x", "y"]);
    }

    #[tokio::test]
    async fn test_json_parse_bad_json() {
        let tool = JsonParseTool::new();
        let result = tool.invoke(json!({"text": "{bad json"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_toml_parse_roundtrip() {
        let tool = TomlParseTool::new();
        let text = r#"
title = "Forge"

[owner]
name = "main"

[deps]
serde = "1"
"#;
        let result = tool.invoke(json!({"text": text})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["json"]["title"], "Forge");
        assert_eq!(result["json"]["owner"]["name"], "main");
        let sections: Vec<String> = result["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(sections.contains(&"title".to_string()));
        assert!(sections.contains(&"owner".to_string()));
        assert!(sections.contains(&"deps".to_string()));
    }

    #[tokio::test]
    async fn test_csv_parse_roundtrip() {
        let tool = CsvParseTool::new();
        let result = tool
            .invoke(json!({"text": "name,age\nalice,30\nbob,25"}))
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        let headers: Vec<String> = result["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(headers, vec!["name", "age"]);
        assert_eq!(result["row_count"], 2);
        assert_eq!(result["rows"][0]["name"], "alice");
        assert_eq!(result["rows"][1]["age"], "25");
    }

    #[tokio::test]
    async fn test_pdf_parse_pending() {
        let tool = PdfParseTool::new();
        let result = tool.invoke(json!({"file_path": "x.pdf"})).await.unwrap();
        assert_eq!(result["status"], "pending_engine");
        assert_eq!(result["tool"], "pdf_parse");
    }

    #[tokio::test]
    async fn test_register_all_5_tools() {
        let router = forge_exec::ToolRouter::new();
        register_all(&router).unwrap();
        let tools = router.list();
        assert_eq!(tools.len(), 5, "register_all 应注册 5 个工具");
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"yaml_parse".to_string()));
        assert!(names.contains(&"json_parse".to_string()));
        assert!(names.contains(&"toml_parse".to_string()));
        assert!(names.contains(&"csv_parse".to_string()));
        assert!(names.contains(&"pdf_parse".to_string()));
    }
}
