use async_trait::async_trait;
use forge_core::{ForgeError, ForgeResult};
use forge_exec::{PermissionLevel, Tool, ToolDescriptor};
use serde_json::json;

fn err(msg: impl Into<String>) -> ForgeError { ForgeError::InvalidState(msg.into()) }

pub struct ReadFileTool { descriptor: ToolDescriptor }
impl ReadFileTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "read_file".into(),
            description: "Read a file from the local filesystem. Returns content with line numbers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to the file" },
                    "offset": { "type": "integer", "description": "Line number to start reading from (0-based)", "default": 0 },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read" }
                },
                "required": ["file_path"]
            }),
            permission: PermissionLevel::ReadOnly,
        }}
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).ok_or_else(|| err("file_path is required"))?;
        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = input.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
        let data = std::fs::read(file_path).map_err(|e| err(format!("Failed to read file: {e}")))?;
        if data.contains(&0) {
            return Ok(json!({ "file_path": file_path, "content": "(binary file)", "size": data.len(), "total_lines": 0 }));
        }
        let text = String::from_utf8_lossy(&data);
        let lines: Vec<&str> = text.split('\n').collect();
        let total_lines = lines.len();
        let end = limit.map(|l| offset + l).unwrap_or(total_lines).min(total_lines);
        let selected: Vec<String> = lines[offset..end].iter().enumerate().map(|(i, line)| format!("{:>4}\t{}", offset + i + 1, line)).collect();
        Ok(json!({ "file_path": file_path, "content": selected.join("\n"), "offset": offset, "total_lines": total_lines, "returned_lines": selected.len() }))
    }
}

pub struct WriteFileTool { descriptor: ToolDescriptor }
impl WriteFileTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "write_file".into(),
            description: "Write content to a file, creating parent directories if needed.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to the file" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["file_path", "content"]
            }),
            permission: PermissionLevel::WorkspaceWrite,
        }}
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).ok_or_else(|| err("file_path is required"))?;
        let content = input.get("content").and_then(|v| v.as_str()).ok_or_else(|| err("content is required"))?;
        if let Some(parent) = std::path::Path::new(file_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| err(format!("Failed to create parent dirs: {e}")))?;
        }
        std::fs::write(file_path, content).map_err(|e| err(format!("Failed to write file: {e}")))?;
        Ok(json!({ "file_path": file_path, "bytes_written": content.len(), "status": "ok" }))
    }
}

pub struct EditFileTool { descriptor: ToolDescriptor }
impl EditFileTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "edit_file".into(),
            description: "Perform exact string replacement in a file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to the file" },
                    "old_string": { "type": "string", "description": "Text to find" },
                    "new_string": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false)" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
            permission: PermissionLevel::WorkspaceWrite,
        }}
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let file_path = input.get("file_path").and_then(|v| v.as_str()).ok_or_else(|| err("file_path is required"))?;
        let old_string = input.get("old_string").and_then(|v| v.as_str()).ok_or_else(|| err("old_string is required"))?;
        let new_string = input.get("new_string").and_then(|v| v.as_str()).ok_or_else(|| err("new_string is required"))?;
        let replace_all = input.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        let content = std::fs::read_to_string(file_path).map_err(|e| err(format!("Failed to read file: {e}")))?;
        let count = content.matches(old_string).count();
        if count == 0 { return Err(err("old_string not found in file")); }
        if count > 1 && !replace_all { return Err(err(format!("old_string appears {count} times, expected exactly 1. Use replace_all=true."))); }
        let new_content = if replace_all { content.replace(old_string, new_string) } else { content.replacen(old_string, new_string, 1) };
        std::fs::write(file_path, &new_content).map_err(|e| err(format!("Failed to write file: {e}")))?;
        Ok(json!({ "file_path": file_path, "replacements": if replace_all { count } else { 1 }, "status": "ok" }))
    }
}

pub struct ExecCommandTool { descriptor: ToolDescriptor }
impl ExecCommandTool {
    pub fn new() -> Self {
        Self { descriptor: ToolDescriptor {
            name: "exec_command".into(),
            description: "Execute a shell command and return stdout/stderr/exit code.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "Command to execute" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (default 120000)" }
                },
                "required": ["cmd"]
            }),
            permission: PermissionLevel::External,
        }}
    }
}

fn read_pipe_to_string<R: std::io::Read>(pipe: &mut R) -> String {
    let mut b = Vec::new();
    match std::io::Read::read_to_end(pipe, &mut b) {
        Ok(_) => String::from_utf8_lossy(&b).to_string(),
        Err(_) => String::new(),
    }
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn descriptor(&self) -> &ToolDescriptor { &self.descriptor }
    async fn invoke(&self, input: serde_json::Value) -> ForgeResult<serde_json::Value> {
        let cmd = input.get("cmd").and_then(|v| v.as_str()).ok_or_else(|| err("cmd is required"))?;
        let timeout_ms = input.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(120_000);
        let joined = tokio::task::spawn_blocking({
            let cmd = cmd.to_string();
            let timeout = std::time::Duration::from_millis(timeout_ms);
            move || {
                let mut child = std::process::Command::new("sh").arg("-c").arg(&cmd)
                    .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
                    .spawn().map_err(|e| err(format!("Failed to spawn: {e}")))?;
                let start = std::time::Instant::now();
                loop {
                    if let Some(status) = child.try_wait().map_err(|e| err(format!("try_wait: {e}")))? {
                        let stdout = child.stdout.take().map(|mut s| read_pipe_to_string(&mut s)).unwrap_or_default();
                        let stderr = child.stderr.take().map(|mut s| read_pipe_to_string(&mut s)).unwrap_or_default();
                        return Ok(json!({ "exit_code": status.code().unwrap_or(-1), "stdout": stdout, "stderr": stderr, "timed_out": false }));
                    }
                    if start.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); return Ok(json!({ "exit_code": -1, "stdout": "", "stderr": "Command timed out", "timed_out": true })); }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }).await
            .map_err(|e: tokio::task::JoinError| err(format!("Task join error: {e}")))?;
        Ok(joined.map_err(|e: ForgeError| e)?)
    }
}

pub fn register_all(router: &forge_exec::ToolRouter) -> ForgeResult<()> {
    router.register(Box::new(ReadFileTool::new()))?;
    router.register(Box::new(WriteFileTool::new()))?;
    router.register(Box::new(EditFileTool::new()))?;
    router.register(Box::new(ExecCommandTool::new()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[tokio::test]
    async fn test_read_file() {
        let dir = tempdir().unwrap(); let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello\nworld\n").unwrap();
        let tool = ReadFileTool::new();
        let result = tool.invoke(json!({ "file_path": file.to_str().unwrap() })).await.unwrap();
        assert!(result["content"].as_str().unwrap().contains("hello"));
        assert_eq!(result["total_lines"], 3);
    }
    #[tokio::test]
    async fn test_write_file() {
        let dir = tempdir().unwrap(); let file = dir.path().join("out.txt");
        let tool = WriteFileTool::new();
        let result = tool.invoke(json!({ "file_path": file.to_str().unwrap(), "content": "abc" })).await.unwrap();
        assert_eq!(result["bytes_written"], 3);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "abc");
    }
    #[tokio::test]
    async fn test_edit_file() {
        let dir = tempdir().unwrap(); let file = dir.path().join("edit.txt");
        std::fs::write(&file, "hello world").unwrap();
        let tool = EditFileTool::new();
        let result = tool.invoke(json!({ "file_path": file.to_str().unwrap(), "old_string": "hello", "new_string": "goodbye" })).await.unwrap();
        assert_eq!(result["replacements"], 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye world");
    }
    #[tokio::test]
    async fn test_edit_file_multiple_no_replace_all() {
        let dir = tempdir().unwrap(); let file = dir.path().join("multi.txt");
        std::fs::write(&file, "aaa aaa").unwrap();
        let tool = EditFileTool::new();
        let result = tool.invoke(json!({ "file_path": file.to_str().unwrap(), "old_string": "aaa", "new_string": "bbb" })).await;
        assert!(result.is_err());
    }
    #[tokio::test]
    async fn test_exec_command() {
        let tool = ExecCommandTool::new();
        let result = tool.invoke(json!({ "cmd": "echo hello" })).await.unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(result["exit_code"], 0);
    }
}
