//! JSON-RPC 2.0 request/response framing for `/mcp`. Split out of [`super`] to keep that file
//! under the size cap — dispatch stays there, this is just the wire format.
//!
//! Ported from `chess-base`'s `server/routes/mcp/rpc.rs`: a malformed body must come back as a
//! *framed* `-32700`/`-32600` JSON-RPC error, not axum's bare-text `400` an ordinary `Json<T>`
//! extractor would produce — see [`super::handle`]'s module doc for why the handler reads raw
//! `Bytes` instead.

use serde::Serialize;
use serde_json::Value;

/// A validated JSON-RPC request. Built by [`parse_request`] from the raw body.
pub(super) struct JsonRpcRequest {
    pub(super) id: Option<Value>,
    pub(super) method: String,
    pub(super) params: Option<Value>,
}

#[derive(Serialize)]
pub(super) struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub(super) struct JsonRpcError {
    pub(super) code: i32,
    pub(super) message: String,
}

impl JsonRpcResponse {
    pub(super) fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(super) fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// Parse the raw request body into a validated [`JsonRpcRequest`], or a framed JSON-RPC error
/// response when it is malformed: invalid JSON maps to `-32700` (parse error), a structurally
/// invalid request to `-32600` (invalid request), echoing the caller's `id` when one can be
/// recovered.
pub(super) fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, JsonRpcResponse> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| JsonRpcResponse::error(None, -32700, "Parse error"))?;

    // Recover the id even from an otherwise-invalid request so the client can correlate the
    // error; a non-scalar id is not a valid id, so drop it.
    let id = match value.get("id") {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Null)) => Some(v.clone()),
        _ => None,
    };

    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(JsonRpcResponse::error(id, -32600, "Invalid Request"));
    }
    let method = match value.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => return Err(JsonRpcResponse::error(id, -32600, "Invalid Request")),
    };

    // Treat an explicit `null` params the same as an absent one.
    let params = value.get("params").filter(|p| !p.is_null()).cloned();

    Ok(JsonRpcRequest { id, method, params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_err(body: &[u8]) -> JsonRpcError {
        parse_request(body)
            .err()
            .expect("expected a framed error")
            .error
            .expect("error envelope")
    }

    #[test]
    fn parse_request_accepts_a_well_formed_request() {
        let Ok(req) = parse_request(br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#) else {
            panic!("expected a valid request");
        };
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(7)));
        assert!(req.params.is_none());
    }

    #[test]
    fn parse_request_maps_invalid_json_to_parse_error() {
        let err = parse_err(b"{not json");
        assert_eq!(err.code, -32700);
    }

    #[test]
    fn parse_request_rejects_a_missing_method_with_invalid_request() {
        let err = parse_request(br#"{"jsonrpc":"2.0","id":3}"#)
            .err()
            .expect("framed error");
        assert_eq!(err.id, Some(json!(3)));
        assert_eq!(err.error.expect("envelope").code, -32600);
    }

    #[test]
    fn parse_request_rejects_a_wrong_jsonrpc_version() {
        assert_eq!(
            parse_err(br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).code,
            -32600
        );
    }

    #[test]
    fn parse_request_rejects_a_non_string_method() {
        assert_eq!(
            parse_err(br#"{"jsonrpc":"2.0","id":1,"method":42}"#).code,
            -32600
        );
    }

    #[test]
    fn parse_request_drops_a_non_scalar_id() {
        let resp = parse_request(br#"{"jsonrpc":"2.0","id":{"a":1},"method":42}"#)
            .err()
            .expect("framed error");
        assert!(resp.id.is_none());
    }

    #[test]
    fn parse_request_treats_null_params_as_absent() {
        let Ok(req) = parse_request(br#"{"jsonrpc":"2.0","id":1,"method":"x","params":null}"#)
        else {
            panic!("expected a valid request");
        };
        assert!(req.params.is_none());
    }
}
