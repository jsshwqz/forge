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
        let parser = pulldown_cmark::Parser::new(&markdown);
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, parser);
        Ok(json!({ "ok": true, "html": html }))
    }
}

// ── text_toon ──

pub struct TextToonTool {
    descriptor: ToolDescriptor,
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
            .split(|c| c == '.' || c == '!' || c == '?' || c == '。' || c == '！' || c == '？')
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

/// 注册所有文本工具到 router（9 工具 = 4 纯 + 4 AI 壳 + 1 text_embed）。
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
    async fn test_register_all_9_tools() {
        let router = forge_exec::ToolRouter::new();
        register_all(&router).unwrap();
        let tools = router.list();
        assert_eq!(tools.len(), 9, "register_all 应注册 9 个工具");
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
    }
}
