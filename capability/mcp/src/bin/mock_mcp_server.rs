//! 测试夹具：最小 MCP stdio server（行分隔 JSON-RPC 2.0）。
//!
//! 支持：initialize / notifications/initialized / tools/list / tools/call(echo)。
//! 仅供 forge-mcp 集成测试通过 CARGO_BIN_EXE 引用，不属于公共 API。

use std::io::{BufRead, Write};

fn respond(id: &serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn respond_error(id: &serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        .to_string()
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned();

        match (method, id) {
            ("initialize", Some(id)) => {
                let r = serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock", "version": "0.1.0" }
                });
                writeln!(out, "{}", respond(&id, r)).unwrap();
                out.flush().unwrap();
            }
            ("tools/list", Some(id)) => {
                let r = serde_json::json!({
                    "tools": [
                        { "name": "echo", "description": "Echo back arguments as text",
                          "inputSchema": { "type": "object" } }
                    ]
                });
                writeln!(out, "{}", respond(&id, r)).unwrap();
                out.flush().unwrap();
            }
            ("tools/call", Some(id)) => {
                let name = v.pointer("/params/name").and_then(|n| n.as_str()).unwrap_or("");
                if name == "echo" {
                    let args = v.pointer("/params/arguments").cloned().unwrap_or(serde_json::Value::Null);
                    let text = args.to_string();
                    let r = serde_json::json!({ "content": [ { "type": "text", "text": text } ] });
                    writeln!(out, "{}", respond(&id, r)).unwrap();
                } else {
                    writeln!(out, "{}", respond_error(&id, -32601, "tool not found")).unwrap();
                }
                out.flush().unwrap();
            }
            (_, None) => { /* 通知：忽略 */ }
            (_, Some(id)) => {
                writeln!(out, "{}", respond_error(&id, -32601, "method not found")).unwrap();
                out.flush().unwrap();
            }
        }
    }
    // EOF：stdin 关闭后自然退出
}
