//! 文本工具：markdown_render / text_toon / text_diff / text_wordcount + 4 AI 壳 + text_embed
//!
//! 批次2（#W-14）。4 纯计算工具 + 4 AI 壳（text_classify/extract/summarize/translate，
//! invoke 返回 pending_engine 降级壳）+ text_embed（pending_engine 占位，批次4 auto_wrap_tool 生成真封装后替换）。

use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::json;

fn err(msg: impl Into<String>) -> ForgeError {
    ForgeError::InvalidState(msg.into())
}

/// 生成 AI 壳的 pending_engine 降级响应（不崩溃不静默，第 3 轮接 OmniRoute Runner 换芯）。
fn pending_engine_response(tool_name: &str) -> serde_json::Value {
    json!({
        "status": "pending_engine",
        "message": "AI engine channel not wired in round 1; tool is a mechanical shell",
        "tool": tool_name
    })
}

// ── markdown_render ──

pub struct MarkdownRenderTool {
    descriptor: ToolDescriptor,
}

impl Default for MarkdownRenderTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "markdown_render".into(),
                description: "将 Markdown 文本渲染为 HTML。输入 {markdown}，返回 {ok, html}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "markdown": { "type": "string", "description": "Markdown 文本" }
                    },
                    "required": ["markdown"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for MarkdownRenderTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let markdown = input
            .get("markdown")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("markdown is required"))?;
        let parser = pulldown_cmark::Parser::new(markdown);
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, parser);
        Ok(json!({ "ok": true, "html": html }))
    }
}

// ── text_toon ──

pub struct TextToonTool {
    descriptor: ToolDescriptor,
}

impl Default for TextToonTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextToonTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_toon".into(),
                description: "将文本压缩为 TOON 格式（token 优化）。输入 {text}，返回 {ok, toon, compressed, original_len, toon_len}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待压缩文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextToonTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        // 简化版 toon：压缩空白 + 引号 → 尖括号
        let toon = text
            .split_whitespace()
            .map(|w| w.trim().trim_matches(|c| c == '"' || c == '\''))
            .collect::<Vec<_>>()
            .join(" ");
        let compressed = toon.len() < text.len();
        Ok(json!({
            "ok": true,
            "toon": toon,
            "compressed": compressed,
            "original_len": text.len(),
            "toon_len": toon.len(),
        }))
    }
}

// ── text_diff ──

pub struct TextDiffTool {
    descriptor: ToolDescriptor,
}

impl Default for TextDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextDiffTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_diff".into(),
                description: "计算两段文本的 unified diff。输入 {a, b}，返回 {ok, unified_diff, changed}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "string", "description": "原文本" },
                        "b": { "type": "string", "description": "新文本" }
                    },
                    "required": ["a", "b"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextDiffTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let a = input
            .get("a")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("a is required"))?;
        let b = input
            .get("b")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("b is required"))?;

        let text_diff = similar::TextDiff::from_lines(a, b);
        let unified = text_diff
            .unified_diff()
            .header("a", "b")
            .to_string();

        Ok(json!({
            "ok": true,
            "unified_diff": unified.trim_end(),
            "changed": unified.contains('+') || unified.contains('-'),
        }))
    }
}

// ── text_wordcount ──

pub struct TextWordcountTool {
    descriptor: ToolDescriptor,
}

impl Default for TextWordcountTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextWordcountTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_wordcount".into(),
                description: "统计文本的词数/句数/行数。输入 {text}，返回 {ok, word_count, sentence_count, line_count}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待统计文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextWordcountTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        let word_count = text.split_whitespace().count();
        let sentence_count = text
            .split(['.', '!', '?', '。', '！', '？'])
            .filter(|s| !s.trim().is_empty())
            .count();
        let line_count = text.lines().count();
        Ok(json!({
            "ok": true,
            "word_count": word_count,
            "sentence_count": sentence_count,
            "line_count": line_count,
        }))
    }
}

// ── 4 AI 壳（pending_engine 降级）──
// 第 3 轮接 OmniRoute Runner 后换芯；round1 仅机械壳，不静默不崩溃。

pub struct TextClassifyTool {
    descriptor: ToolDescriptor,
}

impl Default for TextClassifyTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextClassifyTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_classify".into(),
                description: "对文本分类（round1 AI 壳，pending_engine 降级；第 3 轮接 OmniRoute 引擎后换芯）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待处理文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextClassifyTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(pending_engine_response("text_classify"))
    }
}

pub struct TextExtractTool {
    descriptor: ToolDescriptor,
}

impl Default for TextExtractTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextExtractTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_extract".into(),
                description: "从文本抽取实体（round1 AI 壳，pending_engine 降级；第 3 轮接 OmniRoute 引擎后换芯）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待处理文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextExtractTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(pending_engine_response("text_extract"))
    }
}

pub struct TextSummarizeTool {
    descriptor: ToolDescriptor,
}

impl Default for TextSummarizeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextSummarizeTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_summarize".into(),
                description: "摘要文本（round1 AI 壳，pending_engine 降级；第 3 轮接 OmniRoute 引擎后换芯）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待处理文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextSummarizeTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(pending_engine_response("text_summarize"))
    }
}

pub struct TextTranslateTool {
    descriptor: ToolDescriptor,
}

impl Default for TextTranslateTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextTranslateTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_translate".into(),
                description: "翻译文本（round1 AI 壳，pending_engine 降级；第 3 轮接 OmniRoute 引擎后换芯）。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待处理文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextTranslateTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(pending_engine_response("text_translate"))
    }
}

// ── text_embed（round1 pending_engine 占位，批次4 auto_wrap_tool 生成真封装后替换）──

pub struct TextEmbedTool {
    descriptor: ToolDescriptor,
}

impl Default for TextEmbedTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEmbedTool {
    pub fn new() -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: "text_embed".into(),
                description: "文本向量化（round1 占位，待接 embedding 引擎；批次4 auto_wrap_tool 生成真封装后替换）。输入 {text}。".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "待向量化文本" }
                    },
                    "required": ["text"]
                }),
                permission: PermissionLevel::ReadOnly,
            },
        }
    }
}

#[async_trait]
impl Tool for TextEmbedTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        Ok(pending_engine_response("text_embed"))
    }
}

// ── sanitize：文本净化 ──

pub struct SanitizeTool { descriptor: ToolDescriptor }

impl Default for SanitizeTool {
    fn default() -> Self { Self::new() }
}

impl SanitizeTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "sanitize".into(),
            description: "净化文本：去除控制字符（保留换行/制表），可选 HTML 转义。输入 {text, mode?}，mode=control(默认)|html，返回 {ok, sanitized, removed_chars}。".into(),
            input_schema: json!({
                "type":"object",
                "required":["text"],
                "properties":{
                    "text":{"type":"string"},
                    "mode":{"type":"string","enum":["control","html"]}
                }
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }
}

#[async_trait]
impl Tool for SanitizeTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let text = input.get("text").and_then(|v| v.as_str())
            .ok_or_else(|| err("text is required"))?;
        let mode = input.get("mode").and_then(|v| v.as_str()).unwrap_or("control");
        let cleaned: String = text.chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        let removed = text.chars().count() - cleaned.chars().count();
        let sanitized = if mode == "html" {
            cleaned.replace('&', "&amp;").replace('<', "&lt;")
                .replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
        } else {
            cleaned
        };
        Ok(json!({ "ok": true, "sanitized": sanitized, "removed_chars": removed }))
    }
}

// ── session_report：会话报告 ──

pub struct SessionReportTool { descriptor: ToolDescriptor }

impl Default for SessionReportTool {
    fn default() -> Self { Self::new() }
}

impl SessionReportTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "session_report".into(),
            description: "根据会话事件列表生成统计报告。输入 {events:[{kind, seq?, at?}]}，返回 {ok, report:{total, by_kind, first_at, last_at, summary}}。".into(),
            input_schema: json!({
                "type":"object",
                "required":["events"],
                "properties":{
                    "events":{"type":"array","items":{"type":"object"}}
                }
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }
}

#[async_trait]
impl Tool for SessionReportTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let events = input.get("events").and_then(|v| v.as_array())
            .ok_or_else(|| err("events is required"))?;
        let mut by_kind: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for e in events {
            let kind = e.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            *by_kind.entry(kind.to_string()).or_insert(0) += 1;
        }
        let first_at = events.first().and_then(|e| e.get("at")).cloned().unwrap_or(serde_json::Value::Null);
        let last_at = events.last().and_then(|e| e.get("at")).cloned().unwrap_or(serde_json::Value::Null);
        let summary = format!("{} events across {} kinds", events.len(), by_kind.len());
        Ok(json!({ "ok": true, "report": {
            "total": events.len(),
            "by_kind": by_kind,
            "first_at": first_at,
            "last_at": last_at,
            "summary": summary,
        }}))
    }
}

// ── skill_report：技能报告 ──

pub struct SkillReportTool { descriptor: ToolDescriptor }

impl Default for SkillReportTool {
    fn default() -> Self { Self::new() }
}

impl SkillReportTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "skill_report".into(),
            description: "根据技能清单生成报告。输入 {skills:[{name, capabilities?, version?}]}，返回 {ok, report:{total, names, capabilities_count, summary}}。".into(),
            input_schema: json!({
                "type":"object",
                "required":["skills"],
                "properties":{
                    "skills":{"type":"array","items":{"type":"object"}}
                }
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }
}

#[async_trait]
impl Tool for SkillReportTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let skills = input.get("skills").and_then(|v| v.as_array())
            .ok_or_else(|| err("skills is required"))?;
        let names: Vec<&str> = skills.iter()
            .filter_map(|s| s.get("name").and_then(|v| v.as_str())).collect();
        let cap_count: usize = skills.iter()
            .filter_map(|s| s.get("capabilities").and_then(|v| v.as_array()).map(|a| a.len()))
            .sum();
        let summary = format!("{} skills, {} capabilities", skills.len(), cap_count);
        Ok(json!({ "ok": true, "report": {
            "total": skills.len(),
            "names": names,
            "capabilities_count": cap_count,
            "summary": summary,
        }}))
    }
}

/// 注册所有文本工具到 router（12 工具 = 9 既有 + sanitize + session_report + skill_report）。
pub fn register_all(router: &forge_exec::ToolRouter) -> ForgeResult<()> {
    router.register(Box::new(MarkdownRenderTool::new()))?;
    router.register(Box::new(TextToonTool::new()))?;
    router.register(Box::new(TextDiffTool::new()))?;
    router.register(Box::new(TextWordcountTool::new()))?;
    router.register(Box::new(TextClassifyTool::new()))?;
    router.register(Box::new(TextExtractTool::new()))?;
    router.register(Box::new(TextSummarizeTool::new()))?;
    router.register(Box::new(TextTranslateTool::new()))?;
    router.register(Box::new(TextEmbedTool::new()))?;
    router.register(Box::new(SanitizeTool::new()))?;
    router.register(Box::new(SessionReportTool::new()))?;
    router.register(Box::new(SkillReportTool::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_markdown_render_roundtrip() {
        let tool = MarkdownRenderTool::new();
        let result = tool.invoke(json!({"markdown": "# Hello\n\n- item1\n- item2"})).await.unwrap();
        assert_eq!(result["ok"], true);
        let html: String = result["html"].as_str().unwrap().into();
        assert!(html.contains("<h1>"), "html should contain <h1>, got: {html}");
        assert!(html.contains("<li>"), "html should contain <li>");
    }

    #[tokio::test]
    async fn test_text_toon_roundtrip() {
        let tool = TextToonTool::new();
        let result = tool.invoke(json!({"text": "\"hello world\"  foo bar"})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["compressed"] == true || result["compressed"] == false);
        let original_len: usize = result["original_len"].as_u64().unwrap() as usize;
        let toon_len: usize = result["toon_len"].as_u64().unwrap() as usize;
        assert!(toon_len <= original_len, "toon should be compressed or same");
    }

    #[tokio::test]
    async fn test_text_diff_roundtrip() {
        let tool = TextDiffTool::new();
        let result = tool.invoke(json!({"a": "line1\nline2", "b": "line1\nline3"})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["changed"], true);
        let diff: String = result["unified_diff"].as_str().unwrap().into();
        assert!(diff.contains("-line2"), "diff should contain -line2: {diff}");
        assert!(diff.contains("+line3"), "diff should contain +line3: {diff}");
    }

    #[tokio::test]
    async fn test_text_wordcount_roundtrip() {
        let tool = TextWordcountTool::new();
        let result = tool.invoke(json!({"text": "Hello world. This is a test!"})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["word_count"], 6);
        assert_eq!(result["line_count"], 1);
    }

    #[tokio::test]
    async fn test_text_classify_pending_engine() {
        let tool = TextClassifyTool::new();
        let result = tool.invoke(json!({"text": "some text"})).await.unwrap();
        assert_eq!(result["status"], "pending_engine");
        assert_eq!(result["tool"], "text_classify");
    }

    #[tokio::test]
    async fn test_text_embed_pending_engine() {
        let tool = TextEmbedTool::new();
        let result = tool.invoke(json!({"text": "embed me"})).await.unwrap();
        assert_eq!(result["status"], "pending_engine");
        assert_eq!(result["tool"], "text_embed");
    }

    #[tokio::test]
    async fn test_register_all_12_tools() {
        let router = forge_exec::ToolRouter::new();
        register_all(&router).unwrap();
        let tools = router.list();
        assert_eq!(tools.len(), 12, "register_all 应注册 12 个工具");
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"markdown_render".to_string()));
        assert!(names.contains(&"text_toon".to_string()));
        assert!(names.contains(&"text_diff".to_string()));
        assert!(names.contains(&"text_wordcount".to_string()));
        assert!(names.contains(&"text_classify".to_string()));
        assert!(names.contains(&"text_extract".to_string()));
        assert!(names.contains(&"text_summarize".to_string()));
        assert!(names.contains(&"text_translate".to_string()));
        assert!(names.contains(&"text_embed".to_string()));
        assert!(names.contains(&"sanitize".to_string()));
        assert!(names.contains(&"session_report".to_string()));
        assert!(names.contains(&"skill_report".to_string()));
    }

    #[tokio::test]
    async fn test_sanitize_removes_control() {
        let tool = SanitizeTool::new();
        let result = tool.invoke(json!({"text": "a\u{0001}b\nc"})).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["sanitized"], "ab\nc");
        assert_eq!(result["removed_chars"], 1);
    }
    #[tokio::test]
    async fn test_sanitize_html_mode() {
        let tool = SanitizeTool::new();
        let result = tool.invoke(json!({"text": "<a> & \"b\"", "mode": "html"})).await.unwrap();
        assert_eq!(result["sanitized"], "&lt;a&gt; &amp; &quot;b&quot;");
    }
    #[tokio::test]
    async fn test_session_report() {
        let tool = SessionReportTool::new();
        let result = tool.invoke(json!({
            "events": [
                {"kind": "PlanCreated", "seq": 1, "at": "t1"},
                {"kind": "ActionDispatched", "seq": 2, "at": "t2"},
                {"kind": "Completed", "seq": 3, "at": "t3"}
            ]
        })).await.unwrap();
        assert_eq!(result["report"]["total"], 3);
        assert_eq!(result["report"]["by_kind"]["Completed"], 1);
        assert_eq!(result["report"]["first_at"], "t1");
        assert_eq!(result["report"]["last_at"], "t3");
    }
    #[tokio::test]
    async fn test_skill_report() {
        let tool = SkillReportTool::new();
        let result = tool.invoke(json!({
            "skills": [
                {"name": "code", "capabilities": ["write", "edit"]},
                {"name": "search", "capabilities": ["grep"]}
            ]
        })).await.unwrap();
        assert_eq!(result["report"]["total"], 2);
        assert_eq!(result["report"]["capabilities_count"], 3);
    }
}
