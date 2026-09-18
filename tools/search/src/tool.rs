use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::json;
use std::path::Path;

fn err(msg: impl Into<String>) -> ForgeError { ForgeError::InvalidState(msg.into()) }

pub struct GrepTool { descriptor: ToolDescriptor }

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "grep".into(),
            description: "Search file contents with a regex pattern.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "File or directory to search in (default: current dir)" },
                    "glob": { "type": "string", "description": "File filter pattern, e.g. '*.rs'" },
                    "case_insensitive": { "type": "boolean", "description": "Case insensitive search (default false)" }
                },
                "required": ["pattern"]
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }

    fn search_file(&self, file: &Path, re: &regex::Regex, results: &mut Vec<serde_json::Value>) -> ForgeResult<()> {
        let data = std::fs::read(file).map_err(|e| err(format!("Failed to read {}: {e}", file.display())))?;
        if data.contains(&0) { return Ok(()); }
        let text = String::from_utf8_lossy(&data);
        for (i, line) in text.split('\n').enumerate() {
            if re.is_match(line) {
                results.push(json!({ "file": file.to_string_lossy(), "line": i + 1, "content": line }));
            }
        }
        Ok(())
    }

    fn search_dir(&self, dir: &Path, re: &regex::Regex, glob_filter: &Option<glob::Pattern>, results: &mut Vec<serde_json::Value>) -> ForgeResult<()> {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            let entry = std::fs::read_dir(&current).map_err(|e| err(format!("Failed to read dir {}: {e}", current.display())))?;
            for entry in entry.flatten() {
                let file_type = entry.file_type().map_err(|e| err(format!("Failed to get file_type: {e}")))?;
                if file_type.is_dir() {
                    stack.push(entry.path());
                } else if file_type.is_file() {
                    if let Some(pat) = glob_filter {
                        let file_name_owned = entry.file_name().to_string_lossy().to_string();
                        if !pat.matches(&file_name_owned) { continue; }
                    }
                    let _ = self.search_file(&entry.path(), re, results);
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let pattern = input.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| err("pattern is required"))?;
        let path_str = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob_pattern = input.get("glob").and_then(|v| v.as_str());
        let case_insensitive = input.get("case_insensitive").and_then(|v| v.as_bool()).unwrap_or(false);
        let regex_str = if case_insensitive { format!("(?i){}", regex::escape(pattern)) } else { regex::escape(pattern) };
        let re = regex::Regex::new(&regex_str).map_err(|e| err(format!("Invalid regex pattern: {e}")))?;
        let glob_filter = glob_pattern.map(glob::Pattern::new).transpose().map_err(|e| err(format!("Invalid glob pattern: {e}")))?;
        let mut results = Vec::new();
        let path = Path::new(path_str);
        if path.is_file() {
            self.search_file(path, &re, &mut results)?;
        } else if path.is_dir() {
            self.search_dir(path, &re, &glob_filter, &mut results)?;
        } else {
            return Err(err(format!("Path not found: {}", path.display())));
        }
        Ok(json!({ "pattern": pattern, "path": path_str, "matches": results, "match_count": results.len() }))
    }
}

pub struct GlobTool { descriptor: ToolDescriptor }

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "glob".into(),
            description: "Find files matching a glob pattern.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. '**/*.rs'" },
                    "path": { "type": "string", "description": "Root directory to search from (default: current dir)" }
                },
                "required": ["pattern"]
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let pattern = input.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| err("pattern is required"))?;
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let full_pattern = if pattern.starts_with("**") { format!("{}{}", path, pattern) } else { format!("{}/{}", path, pattern) };
        let entries = glob::glob(&full_pattern).map_err(|e| err(format!("Invalid glob pattern: {e}")))?;
        let mut files = Vec::new();
        for entry in entries.flatten() { files.push(entry.to_string_lossy().to_string()); }
        files.sort();
        Ok(json!({ "pattern": pattern, "path": path, "files": files, "file_count": files.len() }))
    }
}

pub fn register_all(router: &forge_exec::ToolRouter) -> ForgeResult<()> {
    router.register(Box::new(GrepTool::new()))?;
    router.register(Box::new(GlobTool::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[tokio::test]
    async fn test_grep_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world\nfoo bar\nhello again\n").unwrap();
        let tool = GrepTool::new();
        let result = tool.invoke(json!({ "pattern": "hello", "path": file.to_str().unwrap() })).await.unwrap();
        assert_eq!(result["match_count"], 2);
    }
    #[tokio::test]
    async fn test_glob() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let tool = GlobTool::new();
        let result = tool.invoke(json!({ "pattern": "*.rs", "path": dir.path().to_str().unwrap() })).await.unwrap();
        assert_eq!(result["file_count"], 1);
        assert!(result["files"][0].as_str().unwrap().ends_with("a.rs"));
    }
}

