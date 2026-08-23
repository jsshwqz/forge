//! JSON-RPC 2.0 消息构造与解析（MCP stdio 传输用，行分隔）。
//!
//! 仅覆盖客户端所需的最小面：请求/通知构造、响应/其他消息判别。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP 协议版本（2024-11-05 规范）。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// 客户端 → 服务端请求。
#[derive(Serialize)]
pub struct JsonRpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<&'a Value>,
}

/// 客户端 → 服务端通知（无 id）。
#[derive(Serialize)]
pub struct JsonRpcNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
}

/// 服务端返回的错误对象。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// 从服务端读到的一行消息的判别结果。
#[derive(Debug)]
pub enum Incoming {
    /// 对客户端请求的响应（id 匹配由调用方完成）。
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<JsonRpcError>,
    },
    /// 服务端通知或其他消息（本阶段忽略）。
    Other,
}

/// 解析一行 JSON；非对象或解析失败返回 None（调用方跳过）。
pub fn parse_incoming(line: &str) -> Option<Incoming> {
    let v: Value = serde_json::from_str(line).ok()?;
    if !v.is_object() {
        return None;
    }
    let id = v.get("id").and_then(|x| x.as_u64());
    let has_method = v.get("method").is_some();
    match (id, has_method) {
        (Some(id), false) => Some(Incoming::Response {
            id,
            result: v.get("result").cloned(),
            error: v
                .get("error")
                .and_then(|e| serde_json::from_value(e.clone()).ok()),
        }),
        _ => Some(Incoming::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_with_id_and_params() {
        let params = serde_json::json!({"a": 1});
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 7,
            method: "initialize",
            params: Some(&params),
        };
        let s = serde_json::to_string(&req).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["params"]["a"], 1);
    }

    #[test]
    fn request_without_params_omits_field() {
        let req: JsonRpcRequest = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list",
            params: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("params"));
    }

    #[test]
    fn parse_response_with_result() {
        let inc = parse_incoming(r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#).unwrap();
        match inc {
            Incoming::Response { id, result, error } => {
                assert_eq!(id, 3);
                assert_eq!(result.unwrap()["ok"], true);
                assert!(error.is_none());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn parse_response_with_error() {
        let inc = parse_incoming(
            r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601,"message":"method not found"}}"#,
        )
        .unwrap();
        match inc {
            Incoming::Response { id, error, .. } => {
                assert_eq!(id, 4);
                assert_eq!(error.unwrap().code, -32601);
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn parse_notification_is_other() {
        let inc = parse_incoming(r#"{"jsonrpc":"2.0","method":"notify","params":{}}"#).unwrap();
        assert!(matches!(inc, Incoming::Other));
    }

    #[test]
    fn parse_server_request_is_other() {
        // 服务端发起的请求（带 method + id）本阶段按 Other 忽略
        let inc = parse_incoming(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#).unwrap();
        assert!(matches!(inc, Incoming::Other));
    }

    #[test]
    fn parse_garbage_is_none() {
        assert!(parse_incoming("not json").is_none());
    }
}
