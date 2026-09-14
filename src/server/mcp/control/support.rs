//! Shared plumbing every control-plane resource module uses: turning a tool call's JSON
//! `arguments` into a typed request body, and turning a [`ApiError`] into MCP's `isError: true`
//! tool-call content — never a JSON-RPC protocol error, since a validation failure or a
//! not-found slug is a fact about *this call*, not about the request's own well-formedness (the
//! same split [`super::super::handlers`]'s module doc draws for the data plane).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::http::redact_message;
use crate::server::error::ApiError;

use super::ToolOutcome;

/// Deserializes a tool call's `arguments` object into `T`, or a `ToolOutcome::error` describing
/// what's wrong — a malformed argument is a fact about this call, so it becomes content a model
/// can read and correct from, not a JSON-RPC-level rejection.
pub(super) fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, ToolOutcome> {
    serde_json::from_value(args).map_err(|e| ToolOutcome::error(format!("invalid arguments: {e}")))
}

/// Renders an [`ApiError`] with exactly the JSON body `server::error::ApiError`'s own
/// `IntoResponse` would send over HTTP (same shape, same redaction) — a definition rejected over
/// `/mcp` reads identically to one rejected over `/api` (this crate's own "invalid definitions
/// are rejected with the full validation error list, exactly as `/api` does" requirement).
pub(super) fn api_error_outcome(err: ApiError) -> ToolOutcome {
    let body = match err {
        ApiError::Validation(errors) => {
            let redacted: Vec<String> = errors.iter().map(|e| redact_message(e)).collect();
            serde_json::json!({ "errors": redacted })
        }
        ApiError::NotFound(m) => serde_json::json!({ "error": redact_message(&m) }),
        ApiError::BadRequest(m) => serde_json::json!({ "error": redact_message(&m) }),
        ApiError::Forbidden(m) => serde_json::json!({ "error": redact_message(&m) }),
        ApiError::Internal => serde_json::json!({ "error": "internal error" }),
    };
    ToolOutcome::error(body.to_string())
}

/// A successful tool result: `value` serialized as the outcome's text. `unwrap_or_default` (never
/// `.unwrap()`/`.expect()`) because every type actually passed here is one of this crate's own
/// `Serialize` view structs over already-validated data — serialization cannot fail in practice,
/// but this is not a compiler-provable invariant, so it degrades to an empty body instead of
/// panicking on the day that stops being true.
pub(super) fn ok_value(value: impl Serialize) -> ToolOutcome {
    ToolOutcome::ok(serde_json::to_string(&value).unwrap_or_default())
}

/// Collapses a `Result<impl Serialize, ApiError>` into a [`ToolOutcome`] — the shared tail of
/// almost every resource module's `dispatch` arm.
pub(super) fn outcome_of<T: Serialize>(result: Result<T, ApiError>) -> ToolOutcome {
    match result {
        Ok(value) => ok_value(value),
        Err(err) => api_error_outcome(err),
    }
}

/// `*.get`/`*.delete`'s whole argument shape: just the slug naming which definition.
#[derive(serde::Deserialize)]
pub(super) struct SlugArgs {
    pub(super) slug: String,
}
