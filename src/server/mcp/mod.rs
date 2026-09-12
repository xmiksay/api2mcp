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
//!
//! **Endpoint grants.** [`resolve_plan`] is the one place an endpoint slug turns into a
//! plan for every method that needs one (`initialize`, `tools/list`, `tools/call`), so it is
//! also the one place a service token's endpoint restriction
//! (`server::auth::authenticate_mcp`'s second return value) is enforced — not a per-handler
//! check a later sibling method could forget. A restricted token naming some other slug gets
//! exactly [`build_endpoint_not_found`]'s response: the same shape [`crate::resolve::build_plan`]
//! itself produces for a slug with no row at all. That indistinguishability is the point — a
//! `403`-shaped "exists but you can't reach it" would tell an attacker which slugs to go after.

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
use crate::server::auth::{BearerChallenge, EndpointGrants, authenticate_mcp};
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
    let (caller, endpoint_grants) = match authenticate_mcp(&state.db, &state.cfg, &headers).await {
        Ok(pair) => pair,
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

    let resp = dispatch_method(&state, &endpoint_slug, &caller, &endpoint_grants, req).await;
    (StatusCode::OK, Json(resp)).into_response()
}

async fn dispatch_method(
    state: &AppState,
    endpoint_slug: &str,
    caller: &Caller,
    endpoint_grants: &EndpointGrants,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            match resolve_plan(state, endpoint_slug, caller, endpoint_grants, &req.id).await {
                Ok(plan) => JsonRpcResponse::success(req.id, initialize_result(&plan)),
                Err(resp) => resp,
            }
        }
        "tools/list" | "tools/call" => {
            // No scope check: `caller` already passed `authenticate_mcp` to get here — an
            // OAuth access token or a resolved, unrevoked, unexpired service token, either of
            // which *is* "may call tools" in full. The admin-only service token that used to
            // fail a narrower check here is gone (Decision 1; see `server::identity`'s module
            // doc) — there is no longer a distinct credential kind to exclude. The endpoint
            // restriction (as opposed to a blanket "may call tools") is enforced inside
            // `resolve_plan`, below.
            let plan =
                match resolve_plan(state, endpoint_slug, caller, endpoint_grants, &req.id).await {
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

/// Parses `endpoint_slug`, checks it against `endpoint_grants` (empty = every endpoint), and
/// resolves it through `state.plans` (the compiled-plan cache) — or builds the framed JSON-RPC
/// error for whichever of the three ways this can fail: a bad slug (`-32602`, a request
/// problem), a slug outside a restricted token's grant list, or a resolve failure (`-32001`
/// either way — see [`build_endpoint_not_found`] and this module's own doc for why a grant
/// miss must look exactly like a missing endpoint, never a distinct "forbidden").
async fn resolve_plan(
    state: &AppState,
    endpoint_slug: &str,
    caller: &Caller,
    endpoint_grants: &EndpointGrants,
    id: &Option<Value>,
) -> Result<Arc<EndpointPlan>, JsonRpcResponse> {
    let slug: Slug = endpoint_slug.parse().map_err(|e| {
        JsonRpcResponse::error(
            id.clone(),
            -32602,
            format!("invalid endpoint {endpoint_slug:?}: {e}"),
        )
    })?;
    // Indistinguishable from a nonexistent endpoint on purpose: "exists but your token cannot
    // reach it" tells a caller exactly which endpoint to go after.
    if !endpoint_grants.allows(&slug) {
        return Err(build_endpoint_not_found(id, &slug));
    }
    // Ownership and endpoint grants compose: `caller.id` (the token's or session's owner) scopes
    // the lookup to that owner's own definitions, so another owner's endpoint of the same slug
    // resolves to exactly the same not-found response as one that doesn't exist at all.
    state
        .plans
        .get_or_build(&state.stores(), caller.id, &slug)
        .await
        .map_err(|e| JsonRpcResponse::error(id.clone(), -32001, redact_message(&e.to_string())))
}

/// The same `-32001` shape [`crate::resolve::build_plan`] produces for a slug with no
/// `endpoints` row at all (`ResolveError::EndpointNotFound`'s `Display` text, reproduced here
/// rather than imported since nothing about a grant miss ever reaches `resolve::build_plan` —
/// it is rejected one step earlier, in [`resolve_plan`]).
fn build_endpoint_not_found(id: &Option<Value>, slug: &Slug) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id.clone(),
        -32001,
        redact_message(&format!("endpoint {:?} does not exist", slug.as_str())),
    )
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

    /// Pins the whole point of [`build_endpoint_not_found`]: its message must read exactly
    /// like `resolve::ResolveError::EndpointNotFound`'s own `Display` text, or a restricted
    /// token's rejection becomes distinguishable from a genuinely missing endpoint. Compares
    /// through the serialized wire shape rather than a private field — `JsonRpcResponse`'s
    /// fields are only visible inside `rpc`, its defining module.
    #[test]
    fn endpoint_not_found_message_matches_resolve_errors_own_wording() {
        let slug: Slug = "some-slug".parse().unwrap();
        let resp = build_endpoint_not_found(&Some(json!(1)), &slug);
        let expected = crate::resolve::ResolveError::EndpointNotFound {
            slug: slug.as_str().to_owned(),
        }
        .to_string();
        let wire = serde_json::to_value(&resp).unwrap();
        assert_eq!(wire["error"]["message"], json!(expected));
        assert_eq!(wire["error"]["code"], json!(-32001));
    }

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
