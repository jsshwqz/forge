//! MCP stdio server：把 forge-cli 内置工具集以 MCP（行分隔 JSON-RPC 2.0）对外暴露。
//!
//! 协议版本：`PROTOCOL_VERSION`（2024-11-05，与 forge-mcp 客户端一致）。
//! 传输：stdin/stdout 行分隔 JSON；日志只走 stderr，stdout 仅写协议帧。
//!
//! 架构（为什么读在独立线程）：
//! 主线程若同时阻塞读 stdin 行 + 同步写 stdout，在 Windows 管道下会因
//! "同一线程既在 stdin 阻塞等待、又需 flush stdout"产生帧饥饿（initialize
//! 之后的 tools/list 帧被吞）。故 I/O 模型固定为：
//!   - 独立"读线程"：逐行读 stdin，发 (req 帧) 给主线程
//!   - 主线程：处理协议，发 (resp 帧) 给"写线程"
//!   - 独立"写线程"：从队列取帧，`writeln! + flush` 立即落管道
//! stdin EOF 时读线程关闭 req 通道，主线程处理完在途帧后退出，写线程收完
//! 剩余帧后退出，进程自然结束。

use forge_exec::{EchoTool, ToolRouter};
use forge_mcp::PROTOCOL_VERSION;
use forge_worklog::register_all as register_worklog;
use forge_tools_fs::register_all as register_fs;
use forge_tools_search::register_all as register_search;
use forge_tools_parsing::register_all as register_parsing;
use forge_tools_text::register_all as register_text;
use forge_tools_metatool::register_all as register_metatool;
use serde_json::Value;

/// JSON-RPC 2.0 标准错误码。
const ERR_PARSE: i64 = -32700;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_INTERNAL: i64 = -32603;

/// MCP server 主入口：装配 读线程 → 主处理循环 → 写线程 的管线。
pub fn run() {
    let router = build_router();

    // 读线程 → 主线程：原始行（解析在主线程做，便于 parse error 帧统一出口）
    let (req_tx, req_rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(std::io::stdin());
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if req_tx.send(Some(trimmed)).is_err() {
                        break; // 主线程已退出
                    }
                }
                Err(_) => break,
            }
        }
        // 通知主线程 stdin 结束
        req_tx.send(None).ok();
    });

    // 主线程 → 写线程：协议帧
    let (resp_tx, resp_rx) = std::sync::mpsc::channel::<String>();
    let writer_handle = std::thread::spawn(move || {
        use std::io::Write;
        let mut out = std::io::stdout();
        for frame in resp_rx {
            if let Err(e) = writeln!(out, "{frame}") {
                eprintln!("[mcp-server] stdout write failed: {e}");
                break;
            }
            if let Err(e) = out.flush() {
                eprintln!("[mcp-server] stdout flush failed: {e}");
                break;
            }
        }
    });

    // 主处理循环
    for raw in req_rx {
        let Some(content) = raw else {
            break; // stdin EOF
        };
        let req: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                let frame = serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": ERR_PARSE, "message": format!("parse error: {e}") },
                });
                resp_tx.send(frame.to_string()).ok();
                continue;
            }
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned();
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let resp = match method {
            "initialize" => handle_initialize(&id),
            "notifications/initialized" => None, // notification：无响应
            "tools/list" => handle_tools_list(&id, &router),
            "tools/call" => handle_tools_call(&id, &params, &router),
            "ping" => match &id {
                Some(i) => Some(respond_frame(i, Value::Null)),
                None => None,
            },
            other => match &id {
                Some(i) => Some(error_frame(i, ERR_METHOD_NOT_FOUND, format!("method not found: {other}"))),
                None => {
                    eprintln!("[mcp-server] unknown notification: {other}");
                    None
                }
            },
        };
        if let Some(frame) = resp {
            resp_tx.send(frame).ok();
        }
    }

    // 主循环结束：drop resp_tx 关闭通道，join 写线程确保所有在途帧写完。
    // 不能用 std::process::exit(0)（会杀掉写线程导致最后一帧丢失）。
    drop(resp_tx);
    let _ = writer_handle.join();
}

/// 构建工具路由表：内置 EchoTool + worklog 三工具（status/append/export）。
/// 后续接入更多工具集时，在此追加 `register_xxx(router)` 即可。
fn build_router() -> ToolRouter {
    let router = ToolRouter::new();
    router.register(Box::new(EchoTool::new())).expect("echo tool register");
    register_worklog(&router).expect("worklog tools register");
    register_fs(&router).expect("fs tools register");
    register_search(&router).expect("search tools register");
    register_parsing(&router).expect("parsing tools register");
    register_text(&router).expect("text tools register");
    register_metatool(&router).expect("metatool tools register");
    router
}

/// 构造响应帧 `{"jsonrpc":"2.0","id":…,"result":…}`。
fn respond_frame(id: &Value, result: Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .unwrap_or_else(|e| {
        eprintln!("[mcp-server] respond serialize failed: {e}");
        serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"serialize error"}}).to_string()
    })
}

/// 构造错误帧 `{"jsonrpc":"2.0","id":…,"error":{code,message}}`。
fn error_frame(id: &Value, code: i64, message: String) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
    .unwrap_or_else(|e| {
        eprintln!("[mcp-server] error serialize failed: {e}");
        format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32603,"message":"serialize error"}}}}"#)
    })
}

/// initialize 握手：返回完整 result。
fn handle_initialize(id: &Option<Value>) -> Option<String> {
    let id = id.as_ref()?;
    Some(respond_frame(
        id,
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": { "name": "forge-cli", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "tools": { "listChanged": false } },
        }),
    ))
}

/// tools/list：输出 MCP 工具数组（name / description / inputSchema）。
fn handle_tools_list(id: &Option<Value>, router: &ToolRouter) -> Option<String> {
    let id = id.as_ref()?;
    let tools: Vec<Value> = router
        .list()
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "description": d.description,
                "inputSchema": d.input_schema,
            })
        })
        .collect();
    Some(respond_frame(id, serde_json::json!({ "tools": tools })))
}

/// tools/call：按 name 路由调用，结果包进 MCP content 块。
/// 未知工具 → -32602；执行错误 → -32603。
///
/// 在独立线程内新建 current-thread runtime 驱动异步工具，主线程经 mpsc 取结果。
fn handle_tools_call(id: &Option<Value>, params: &Value, router: &ToolRouter) -> Option<String> {
    let id = id.as_ref()?;
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    if name.is_empty() {
        return Some(error_frame(id, ERR_INVALID_PARAMS, "missing tool name".to_string()));
    }

    let tool = match router.route(name) {
        Ok(t) => t,
        Err(_) => {
            return Some(error_frame(id, ERR_INVALID_PARAMS, format!("tool not found: {name}")));
        }
    };

    enum CallOutcome {
        Ok(Value),
        Err(String),
    }
    let (tx, rx) = std::sync::mpsc::channel::<CallOutcome>();
    std::thread::spawn(move || {
        let outcome = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Err(e) => CallOutcome::Err(format!("runtime build failed: {e}")),
            Ok(rt) => match rt.block_on(tool.invoke(args)) {
                Ok(v) => CallOutcome::Ok(v),
                Err(e) => CallOutcome::Err(format!("tool invoke failed: {e}")),
            },
        };
        let _ = tx.send(outcome);
    });

    let outcome = match rx.recv() {
        Ok(o) => o,
        Err(_) => {
            return Some(error_frame(id, ERR_INTERNAL, "tool invoke thread lost".to_string()));
        }
    };

    match outcome {
        CallOutcome::Ok(result) => {
            let text = result.to_string();
            Some(respond_frame(
                id,
                serde_json::json!({
                    "content": [ { "type": "text", "text": text } ],
                }),
            ))
        }
        CallOutcome::Err(msg) => Some(error_frame(id, ERR_INTERNAL, msg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_has_echo_and_worklog() {
        let router = build_router();
        let all = router.list();
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"echo"), "echo missing: {names:?}");
        assert!(names.contains(&"worklog_export"), "worklog_export missing: {names:?}");
        assert!(names.contains(&"read_file"), "read_file missing: {names:?}");
        assert!(names.contains(&"write_file"), "write_file missing: {names:?}");
        assert!(names.contains(&"edit_file"), "edit_file missing: {names:?}");
        assert!(names.contains(&"exec_command"), "exec_command missing: {names:?}");
        assert!(names.contains(&"grep"), "grep missing: {names:?}");
        assert!(names.contains(&"glob"), "glob missing: {names:?}");
        assert!(names.contains(&"yaml_parse"), "yaml_parse missing: {names:?}");
        assert!(names.contains(&"json_parse"), "json_parse missing: {names:?}");
        assert!(names.contains(&"toml_parse"), "toml_parse missing: {names:?}");
        assert!(names.contains(&"csv_parse"), "csv_parse missing: {names:?}");
        assert!(names.contains(&"pdf_parse"), "pdf_parse missing: {names:?}");
        assert!(names.contains(&"markdown_render"), "markdown_render missing: {names:?}");
        assert!(names.contains(&"text_toon"), "text_toon missing: {names:?}");
        assert!(names.contains(&"text_diff"), "text_diff missing: {names:?}");
        assert!(names.contains(&"text_wordcount"), "text_wordcount missing: {names:?}");
        assert!(names.contains(&"text_classify"), "text_classify missing: {names:?}");
        assert!(names.contains(&"text_embed"), "text_embed missing: {names:?}");
        assert_eq!(all.len(), 25, "expected 25 tools, got {names:?}");
    }

    #[test]
    fn test_initialize_response_shape() {
        let id = Some(Value::from(1));
        let frame = handle_initialize(&id).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["result"]["serverInfo"]["name"], "forge-cli");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn test_tools_list_shape() {
        let router = build_router();
        let id = Some(Value::from(2));
        let frame = handle_tools_list(&id, &router).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        let arr = v["result"]["tools"].as_array().unwrap();
        assert_eq!(arr.len(), 25);
        let names: Vec<&str> = arr.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"worklog_status"));
        assert!(arr[0]["inputSchema"].is_object());
        assert_eq!(v["id"], 2);
    }

    #[test]
    fn test_tools_call_missing_name_is_invalid_params() {
        let router = build_router();
        let id = Some(Value::from(3));
        let params = Value::Object(serde_json::Map::new());
        let frame = handle_tools_call(&id, &params, &router).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["error"]["code"], ERR_INVALID_PARAMS);
        assert!(v["error"]["message"].as_str().unwrap().contains("missing tool name"));
    }

    #[test]
    fn test_tools_call_unknown_tool_is_invalid_params() {
        let router = build_router();
        let id = Some(Value::from(4));
        let params = serde_json::json!({ "name": "no_such_tool" });
        let frame = handle_tools_call(&id, &params, &router).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["error"]["code"], ERR_INVALID_PARAMS);
        assert!(v["error"]["message"].as_str().unwrap().contains("tool not found"));
    }

    #[test]
    fn test_tools_call_echo_roundtrip() {
        let router = build_router();
        let id = Some(Value::from(5));
        let params = serde_json::json!({ "name": "echo", "arguments": { "text": "hi" } });
        let frame = handle_tools_call(&id, &params, &router).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["result"]["content"][0]["type"], "text");
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("hi"));
    }

    #[test]
    fn test_unknown_notification_silent() {
        let id: Option<Value> = None;
        let resp = match "frobnicate" {
            other => match &id {
                Some(i) => Some(error_frame(i, ERR_METHOD_NOT_FOUND, format!("method not found: {other}"))),
                None => None,
            },
        };
        assert!(resp.is_none());
    }
}

