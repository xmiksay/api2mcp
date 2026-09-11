//! MCP server: a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp/{endpoint_slug}` (bare
//! `POST /mcp` resolves to `cfg.default_endpoint` — `endpoint` is a first-class entity, and a
//! single fixed path would leave it with no transport).
//!
//! [`handle`] takes raw [`Bytes`], not `Json<T>`: a malformed body must come back as a properly
//! framed JSON-RPC `-32700` ([`rpc::parse_request`]), not axum's bare-text `400` an ordinary
//! extractor would produce on the same input. Authentication also runs *inside* this handler
//! rather than in a middleware, for the same reason — a missing/bad credential becomes a framed
//! JSON-RPC `401` carrying `WWW-Authenticate`, not axum's plain-text rejection.
//!
//! [`handlers`] owns `tools/list`/`tools/call` (the two methods that need a resolved
//! [`crate::resolve::EndpointPlan`]); [`registry`] renders a plan into MCP's tool-descriptor
//! shape; [`invoke`] is the `invoke`/`list_tools` dispatcher; [`rpc`] is the wire format;
//! [`instructions`] is the `initialize` response's static text.

mod handlers;
mod instructions;
mod invoke;
mod registry;
mod rpc;

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::http::redact_message;
use crate::model::Slug;
use crate::resolve::EndpointPlan;
use crate::runtime::Executor;
use crate::server::auth::{BearerChallenge, authenticate_mcp};
use crate::server::identity::Caller;
use crate::server::state::AppState;

use instructions::INSTRUCTIONS;
use rpc::{JsonRpcRequest, JsonRpcResponse, parse_request};

const SERVER_NAME: &str = "api2mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Mounts both MCP routes. Generic over `AppState` rather than taking one — nothing in this
/// module needs its own closure state beyond what `AppState` already carries, unlike (say)
/// `chess-base`'s `McpState`, which wraps a static tool registry this crate builds per-request
/// instead (see `registry`'s module doc for why that's cheap here).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp", post(handle_default))
        .route("/mcp/{endpoint_slug}", post(handle_named))
}

async fn handle_default(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let default = state.cfg.default_endpoint.clone();
    handle(state, default, headers, body).await
}

async fn handle_named(
    State(state): State<AppState>,
    Path(endpoint_slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle(state, endpoint_slug, headers, body).await
}

async fn handle(
    state: AppState,
    endpoint_slug: String,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match authenticate_mcp(&state.db, &state.cfg, &headers).await {
        Ok(caller) => caller,
        Err(challenge) => return unauthorized(challenge),
    };

    let req = match parse_request(&body) {
        Ok(req) => req,
        Err(resp) => return (StatusCode::OK, Json(resp)).into_response(),
    };

    // A notification (no reply expected) per the MCP HTTP transport: acknowledge with 202 and
    // an empty body rather than a JSON-RPC envelope.
    if req.method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }

    let resp = dispatch_method(&state, &endpoint_slug, &caller, req).await;
    (StatusCode::OK, Json(resp)).into_response()
}

async fn dispatch_method(
    state: &AppState,
    endpoint_slug: &str,
    caller: &Caller,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => match resolve_plan(state, endpoint_slug, &req.id).await {
            Ok(plan) => JsonRpcResponse::success(req.id, initialize_result(&plan)),
            Err(resp) => resp,
        },
        "tools/list" | "tools/call" => {
            // No scope check: `caller` already passed `authenticate_mcp` to get here — an
            // OAuth access token or a resolved, unrevoked, unexpired service token, either of
            // which *is* "may call tools" in full. The admin-only service token that used to
            // fail a narrower check here is gone (Decision 1; see `server::identity`'s module
            // doc) — there is no longer a distinct credential kind to exclude.
            let plan = match resolve_plan(state, endpoint_slug, &req.id).await {
                Ok(plan) => plan,
                Err(resp) => return resp,
            };
            if req.method == "tools/list" {
                handlers::tools_list(req.id, &plan)
            } else {
                let executor =
                    Executor::new(state.stores(), state.upstream.clone(), state.ssrf_policy());
                handlers::tools_call(req.id, &plan, &executor, caller, req.params).await
            }
        }
        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
    }
}

/// Parses `endpoint_slug` and resolves it through `state.plans` (the compiled-plan cache), or
/// builds the framed JSON-RPC error for either failure — a bad slug (`-32602`, a request problem)
/// or a resolve failure (`-32001`, this endpoint's own definitions don't compile right now).
async fn resolve_plan(
    state: &AppState,
    endpoint_slug: &str,
    id: &Option<Value>,
) -> Result<Arc<EndpointPlan>, JsonRpcResponse> {
    let slug: Slug = endpoint_slug.parse().map_err(|e| {
        JsonRpcResponse::error(
            id.clone(),
            -32602,
            format!("invalid endpoint {endpoint_slug:?}: {e}"),
        )
    })?;
    state
        .plans
        .get_or_build(&state.stores(), &slug)
        .await
        .map_err(|e| JsonRpcResponse::error(id.clone(), -32001, redact_message(&e.to_string())))
}

/// Builds the `401` response carrying the `WWW-Authenticate` bearer challenge that points an
/// MCP client at `/.well-known/oauth-protected-resource` (chunk C12).
fn unauthorized(challenge: BearerChallenge) -> Response {
    let body = Json(JsonRpcResponse::error(None, -32000, "Unauthorized"));
    let mut response: Response = (StatusCode::UNAUTHORIZED, body).into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge.www_authenticate) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

/// The generic [`INSTRUCTIONS`] text, plus `plan`'s own human-authored
/// `EndpointPlan::instructions` when the definer set one — the one part of `initialize`'s
/// response that's specific to *this* endpoint rather than to api2mcp in general.
fn initialize_result(plan: &EndpointPlan) -> Value {
    let instructions = match &plan.instructions {
        Some(extra) => format!(
            "{INSTRUCTIONS}\n## This endpoint ({})\n\n{extra}\n",
            plan.slug
        ),
        None => INSTRUCTIONS.to_owned(),
    };
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": instructions
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_response_carries_the_www_authenticate_header() {
        let response = unauthorized(BearerChallenge {
            www_authenticate: r#"Bearer resource_metadata="http://h/.well-known/x""#.to_owned(),
        });
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some(r#"Bearer resource_metadata="http://h/.well-known/x""#)
        );
    }
}
