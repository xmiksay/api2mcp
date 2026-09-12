//! `tools/list` and `tools/call` — the two methods that need a resolved [`EndpointPlan`], as
//! opposed to `initialize`/`notifications/initialized`, which [`super`] handles directly. Split
//! out to keep that file under the workspace's file-size cap.

use serde_json::{Value, json};

use crate::http::redact_message;
use crate::resolve::EndpointPlan;
use crate::runtime::Executor;
use crate::server::identity::Caller;
use crate::store::RunCallerKind;

use super::invoke::{self, DispatchError, ToolCallOutcome};
use super::registry;
use super::rpc::JsonRpcResponse;

pub(super) fn tools_list(id: Option<Value>, plan: &EndpointPlan) -> JsonRpcResponse {
    JsonRpcResponse::success(id, registry::tool_list(plan))
}

/// `tools/call`: parses the `{name, arguments}` params object, dispatches through
/// [`invoke::dispatch`], and turns the result into the MCP `content`/`isError` envelope — or, for
/// a protocol-level problem, a JSON-RPC `error` instead. Getting that split backwards (a business
/// failure as a JSON-RPC error, or a genuinely malformed request as `isError: true`) is the most
/// common MCP implementation bug, per the chunk brief this module was built from.
pub(super) async fn tools_call(
    id: Option<Value>,
    plan: &EndpointPlan,
    executor: &Executor,
    caller: &Caller,
    params: Option<Value>,
) -> JsonRpcResponse {
    let Some(params) = params else {
        return JsonRpcResponse::error(id, -32602, "Missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // `server::auth::authenticate_mcp` resolves both an OAuth 2.1 access token and a static
    // service token; an OAuth-authenticated caller comes back as `CallerKind::Session`
    // (`Caller::from_user`), the same kind a browser session gets. This still always records
    // `RunCallerKind::ServiceToken` regardless of which credential resolved the caller.
    let caller_kind = RunCallerKind::ServiceToken;
    let caller_id = caller.id.to_string();

    match invoke::dispatch(executor, plan, name, arguments, caller_kind, caller_id).await {
        Ok(outcome) => JsonRpcResponse::success(id, tool_envelope(outcome)),
        Err(DispatchError::UnknownTool(name)) => {
            JsonRpcResponse::error(id, -32602, format!("Unknown tool: {name}"))
        }
        Err(DispatchError::MissingToolName) => JsonRpcResponse::error(
            id,
            -32602,
            format!(
                "{} requires a `tool_name` argument",
                registry::INVOKE_TOOL_NAME
            ),
        ),
        Err(DispatchError::Internal(message)) => {
            JsonRpcResponse::error(id, -32000, redact_message(&message))
        }
    }
}

/// Wraps a [`ToolCallOutcome`] into the MCP `tools/call` result envelope: `isError` is present
/// (and `true`) only on failure — a successful call omits the key entirely rather than sending
/// `isError: false`, matching how every MCP client actually checks it (`if (result.isError)`).
fn tool_envelope(outcome: ToolCallOutcome) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": outcome.text }]
    });
    if outcome.is_error {
        result["isError"] = json!(true);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_outcome_envelope_omits_is_error() {
        let outcome = ToolCallOutcome {
            text: "fine".to_owned(),
            is_error: false,
        };
        let env = tool_envelope(outcome);
        assert!(env.get("isError").is_none());
        assert_eq!(env["content"][0]["text"], "fine");
    }

    #[test]
    fn error_outcome_envelope_sets_is_error_true() {
        let outcome = ToolCallOutcome {
            text: "boom".to_owned(),
            is_error: true,
        };
        let env = tool_envelope(outcome);
        assert_eq!(env["isError"], json!(true));
        assert_eq!(env["content"][0]["text"], "boom");
    }
}
