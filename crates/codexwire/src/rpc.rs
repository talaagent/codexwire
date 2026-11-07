//! JSON-RPC framing for the app-server: one JSON object per line, no
//! `"jsonrpc"` member (Codex leaves it out). Requests go both ways (the
//! server asks for approvals), so a line is one of four shapes.

use serde_json::{Map, Value, json};

/// A request id: Codex uses integers, the protocol allows strings.
pub type RequestId = Value;

/// One line from the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// The answer to one of our requests.
    Response { id: RequestId, result: Value },
    /// One of our requests failed.
    Error { id: RequestId, error: RpcError },
    /// The server asks us something (approvals); answer with [`response`].
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// The server tells us something.
    Notification { method: String, params: Value },
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl core::fmt::Display for RpcError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

/// Parse one line. Blank lines are `Ok(None)`.
pub fn parse_line(line: &str) -> Result<Option<Incoming>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(line).map_err(|e| format!("bad JSON-RPC line: {e}"))?;
    let Value::Object(mut o) = v else {
        return Err("JSON-RPC line is not an object".into());
    };
    let params = o.remove("params").unwrap_or(Value::Null);
    let method = match o.remove("method") {
        Some(Value::String(m)) => Some(m),
        Some(_) => return Err("JSON-RPC method is not a string".into()),
        None => None,
    };
    let id = o.remove("id");
    Ok(Some(match (method, id) {
        (Some(method), Some(id)) => Incoming::Request { id, method, params },
        (Some(method), None) => Incoming::Notification { method, params },
        (None, Some(id)) => {
            if let Some(e) = o.remove("error") {
                Incoming::Error {
                    id,
                    error: rpc_error(e),
                }
            } else {
                Incoming::Response {
                    id,
                    result: o.remove("result").unwrap_or(Value::Null),
                }
            }
        }
        (None, None) => return Err("JSON-RPC line has neither method nor id".into()),
    }))
}

fn rpc_error(e: Value) -> RpcError {
    let mut o = match e {
        Value::Object(o) => o,
        other => {
            return RpcError {
                code: 0,
                message: other.to_string(),
                data: None,
            };
        }
    };
    RpcError {
        code: o.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: match o.remove("message") {
            Some(Value::String(s)) => s,
            Some(v) => v.to_string(),
            None => String::new(),
        },
        data: o.remove("data"),
    }
}

/// A request line (with its newline).
pub fn request(id: &RequestId, method: &str, params: &Value) -> String {
    line(json!({ "id": id, "method": method, "params": params }))
}

/// A notification line; `params` left out when `None`.
pub fn notification(method: &str, params: Option<&Value>) -> String {
    let mut o = Map::new();
    o.insert("method".into(), method.into());
    if let Some(p) = params {
        o.insert("params".into(), p.clone());
    }
    line(Value::Object(o))
}

/// The answer to a server request.
pub fn response(id: &RequestId, result: &Value) -> String {
    line(json!({ "id": id, "result": result }))
}

/// An error answer to a server request.
pub fn error_response(id: &RequestId, code: i64, message: &str) -> String {
    line(json!({ "id": id, "error": { "code": code, "message": message } }))
}

fn line(v: Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_four_shapes() {
        let r = parse_line(r#"{"id":1,"result":{"userAgent":"x/1"}}"#).unwrap();
        assert_eq!(
            r,
            Some(Incoming::Response {
                id: json!(1),
                result: json!({"userAgent":"x/1"})
            })
        );
        let r = parse_line(r#"{"id":"a","error":{"code":-32600,"message":"no"}}"#).unwrap();
        assert_eq!(
            r,
            Some(Incoming::Error {
                id: json!("a"),
                error: RpcError {
                    code: -32600,
                    message: "no".into(),
                    data: None
                }
            })
        );
        let r = parse_line(r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command"}}"#)
            .unwrap();
        assert!(
            matches!(r, Some(Incoming::Request { id, method, .. }) if id == json!(0) && method == "item/commandExecution/requestApproval")
        );
        let r =
            parse_line(r#"{"method":"turn/started","params":{"threadId":"t"},"emittedAtMs":1}"#)
                .unwrap();
        assert!(
            matches!(r, Some(Incoming::Notification { method, params }) if method == "turn/started" && params["threadId"] == "t")
        );
        assert_eq!(parse_line("  \n").unwrap(), None);
        assert!(parse_line("[1]").is_err());
        assert!(parse_line("{}").is_err());
        assert!(parse_line("{nope").is_err());
    }

    #[test]
    fn writes_lines() {
        assert_eq!(
            request(
                &json!(3),
                "turn/interrupt",
                &json!({"threadId":"t","turnId":"u"})
            ),
            "{\"id\":3,\"method\":\"turn/interrupt\",\"params\":{\"threadId\":\"t\",\"turnId\":\"u\"}}\n"
        );
        assert_eq!(
            notification("initialized", None),
            "{\"method\":\"initialized\"}\n"
        );
        assert_eq!(
            response(&json!(0), &json!({"decision":"decline"})),
            "{\"id\":0,\"result\":{\"decision\":\"decline\"}}\n"
        );
        let e = error_response(&json!(7), -32601, "unsupported");
        assert_eq!(
            parse_line(&e).unwrap(),
            Some(Incoming::Error {
                id: json!(7),
                error: RpcError {
                    code: -32601,
                    message: "unsupported".into(),
                    data: None
                }
            })
        );
    }
}
