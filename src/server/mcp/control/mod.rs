//! The control plane: bare `POST /mcp`, the factory an authoring agent (typically Claude Code)
//! talks to in order to build the curated tools `/mcp/{slug}` then serves. Reachable only by a
//! [`crate::store::ServiceTokenRecord`] minted with `control_plane: true` — see
//! [`crate::server::auth::ResolvedCredential`] and [`super::resolve_control_plane`] for how that
//! capability is checked before any method here runs.
//!
//! **Accepted trade-off, stated plainly.** An agent that can create a [`crate::model::Service`]
//! can create an egress channel: [`services::dispatch`]'s `service.create`/`service.update` take
//! an arbitrary `base_url`/`origin_allowlist`, and any origin they've been sent to becomes a
//! reachable target for a subsequent `api_call` this same agent (or another with the same
//! capability) defines against it. Nothing in this module vets that origin — the user reviews the
//! calls being made, and that review is the control. This means I2's "reachable origins are
//! computable before execution" describes what the definitions on file happen to say, not a
//! guarantee about where an agent *could* eventually route data — an agent could always define a
//! service pointed anywhere. That is the accepted shape of the risk, not a gap to close later
//! (Decision, per the plan authorizing this module).
//!
//! **`auth_provider` is invisible here, entirely — not read-only, absent.** No tool on this
//! surface lists, reads, creates, updates or deletes an auth provider, and none accepts an
//! auth-provider slug as an input field: an api_call names no provider of its own in the first
//! place (a service has at most one, and every api_call on it uses it — see `model::ApiCall`'s
//! doc), so [`api_calls`] has nothing to expose, force or preserve there at all; [`endpoints`]
//! still forces `auth_providers` (an endpoint's own I5 join, `endpoint_auth_providers` — see
//! `store::endpoint`'s module doc) empty on create / preserves it on update, since that scope is
//! independent of Change 1. I5's whole point is that the credential-to-origin binding is set by
//! a human, never proposed, read or moved by an agent, and a read-only list would still leak
//! which credentials exist and what they're bound to — so there is no read surface either.
//!
//! **The consequence, handled deliberately, not left to fall out as a bug:** an api_call created
//! through this surface starts on a service with whatever auth provider (if any) that service
//! already has — there is no per-call wiring for an agent to set or fail to set. That is the
//! intended workflow, not a gap — an agent defines the *shape* of a capability, and a human wires
//! a service's credential up afterwards through `/api` or the CLI, which is exactly what I5
//! reserves for a person. [`INSTRUCTIONS`] and [`api_calls::descriptors`]/
//! [`endpoints::descriptors`]'s own tool descriptions say this plainly, so an agent is told *who*
//! attaches a credential and *why it isn't this tool*, rather than discovering a silent gap or
//! mistaking a resulting 401 for its own error. Calling an api_call on a service with no provider
//! is not a special case at the HTTP layer either — `dispatch` (`runtime::dispatch::AuthProviders`)
//! already sends no credential whenever a service has none, exactly as a human-authored,
//! credential-less service over `/api` always has; the unauthenticated upstream response (a
//! `401`, typically) is recorded and returned like any other non-2xx response, nothing more.
//!
//! Module layout: [`schema`] (shared `inputSchema` fragments), [`support`] (arg parsing +
//! [`crate::server::error::ApiError`]-to-[`ToolOutcome`] rendering), [`registry`] (the static
//! tool descriptor list), then one file per resource — each owns both its tool descriptors and
//! its dispatch arms so the
//! two can never drift apart. Every resource's actual CRUD/validation logic lives in
//! `server::api::*`'s `pub(crate)` `_for_owner` functions (see that module's own doc); this
//! module only adapts JSON-RPC `tool_name`/`arguments` to those calls.

mod api_calls;
mod endpoints;
mod registry;
mod runs_tags;
mod schema;
mod scripts;
mod services;
mod support;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::auth::ResolvedCredential;
use crate::server::state::AppState;

use super::rpc::{JsonRpcRequest, JsonRpcResponse};

const INSTRUCTIONS: &str = "\
# api2mcp — control plane

This is the factory, not a curated tool endpoint: it exists so you can define the services, \
api_calls, scripts and endpoints that `/mcp/{slug}` then serves to a consuming agent. Nothing \
you define here becomes visible anywhere until an `endpoint`'s `tag_expr` selects it.

## The four kinds of definition, and how they relate

- **service** — one upstream HTTP origin: base URL, allowed origins for redirects, timeouts, \
concurrency. Every api_call belongs to exactly one service.
- **api_call** — one HTTP request template against a service: method, path template, params, \
optional response projection. This is what actually becomes an MCP tool.
- **script** — a small Rhai program that composes one or more api_calls (declared in its \
`callable` map) into a single tool, for logic a single HTTP call can't express.
- **endpoint** — a `tag_expr` (e.g. `has(read) and not has(deprecated)`) selecting which \
api_calls/scripts are exposed as tools at `/mcp/{that endpoint's slug}`, plus a write ceiling and \
a budget. A tool becomes visible on an endpoint purely by tag membership — tag an api_call or \
script, then write (or reuse) a `tag_expr` that matches it.

## Narrowing what a caller sees

A **projection** on an api_call rewrites its raw upstream JSON response into a smaller, named \
shape (JSONPath per field) — use it so a tool returns exactly what a calling model needs, not a \
full upstream payload it has to parse itself. **Budgets** (call count, bytes, wall-clock time, \
page count, concurrency) cap what a single run may do; a script's own budget can only narrow its \
endpoint's, never widen it.

## Credentials are not yours to attach

There is no auth-provider tool here at all — not even to list one. An api_call you define has no \
credential attached; a human wires one to it afterwards, outside this API, on purpose (that \
binding is the one thing standing between a credential and an origin it was never meant to \
authenticate against). Calling an api_call before that happens sends no credential and, against \
most real APIs, gets back a 401 — that is expected, not a mistake on your part, and not something \
you can fix from here. Define the api_call's shape (method, path, params, projection) and leave \
the credential to its owner.

## Before you consider a definition done

Use `api_call.test`/`script.test` to actually run it against the real upstream and see both the \
raw response and (for an api_call with a projection) the projected shape side by side. A \
definition that has never been test-run is not done — test before moving on, and again after any \
change to its params, projection or the service it targets.

## Ownership

Every definition you create belongs to this token's owner. `*.list`/`*.get` only ever show that \
owner's own definitions, never another owner's, even one with an identical slug.
";

/// A tool call's result in MCP's `content`/`isError` shape, before JSON-RPC framing — the
/// control-plane analogue of [`super::invoke::ToolCallOutcome`], kept as its own (identically
/// shaped) type rather than imported: the two dispatch trees are otherwise fully independent, and
/// importing one two-field struct across that boundary would buy nothing over restating it.
pub(super) struct ToolOutcome {
    pub(super) text: String,
    pub(super) is_error: bool,
}

impl ToolOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

fn tool_envelope(outcome: ToolOutcome) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": outcome.text }] });
    if outcome.is_error {
        result["isError"] = json!(true);
    }
    result
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": super::PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": super::SERVER_NAME, "version": super::SERVER_VERSION },
        "instructions": INSTRUCTIONS,
    })
}

fn tools_list() -> Value {
    json!({ "tools": registry::descriptors() })
}

/// Tries every resource module's `dispatch` in turn until one recognizes `name`; `None` from all
/// of them is "no such tool", not "no such resource" — the caller renders that as a JSON-RPC
/// `-32602`, matching the data plane's own unknown-tool-name shape.
async fn dispatch(
    state: &AppState,
    owner_id: Uuid,
    name: &str,
    args: Value,
) -> Option<ToolOutcome> {
    if let Some(o) = services::dispatch(state, owner_id, name, args.clone()).await {
        return Some(o);
    }
    if let Some(o) = api_calls::dispatch(state, owner_id, name, args.clone()).await {
        return Some(o);
    }
    if let Some(o) = scripts::dispatch(state, owner_id, name, args.clone()).await {
        return Some(o);
    }
    if let Some(o) = endpoints::dispatch(state, owner_id, name, args.clone()).await {
        return Some(o);
    }
    runs_tags::dispatch(state, owner_id, name, args).await
}

async fn tools_call(
    id: Option<Value>,
    state: &AppState,
    owner_id: Uuid,
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

    match dispatch(state, owner_id, name, arguments).await {
        Some(outcome) => JsonRpcResponse::success(id, tool_envelope(outcome)),
        None => JsonRpcResponse::error(id, -32602, format!("Unknown tool: {name}")),
    }
}

/// Entry point [`super::handle_control_plane`] calls once a caller has already been
/// authenticated — `credential.control_plane` is checked here, not by the caller, so it gates
/// exactly the same three methods (`initialize`, `tools/list`, `tools/call`) [`super::resolve_plan`]
/// gates on the data plane and *only* those: an unrelated method (or a malformed one, handled one
/// level up alongside `notifications/initialized`) still gets its own ordinary response
/// regardless of the capability, matching how a restricted data-plane token's grant check never
/// runs for anything but those same three methods either.
pub(super) async fn dispatch_method(
    state: &AppState,
    credential: &ResolvedCredential,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let owner_id = credential.caller.id;
    match req.method.as_str() {
        "initialize" | "tools/list" | "tools/call" => {
            if let Err(resp) = super::resolve_control_plane(credential, &req.id) {
                return resp;
            }
            match req.method.as_str() {
                "initialize" => JsonRpcResponse::success(req.id, initialize_result()),
                "tools/list" => JsonRpcResponse::success(req.id, tools_list()),
                _ => tools_call(req.id, state, owner_id, req.params).await,
            }
        }
        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_outcome_envelope_omits_is_error() {
        let env = tool_envelope(ToolOutcome::ok("fine"));
        assert!(env.get("isError").is_none());
        assert_eq!(env["content"][0]["text"], "fine");
    }

    #[test]
    fn error_outcome_envelope_sets_is_error_true() {
        let env = tool_envelope(ToolOutcome::error("boom"));
        assert_eq!(env["isError"], json!(true));
    }

    #[test]
    fn tools_list_is_non_empty_and_every_name_is_unique() {
        let list = tools_list();
        let tools = list["tools"].as_array().expect("tools array");
        assert!(!tools.is_empty());
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let unique_count = {
            names.sort_unstable();
            names.dedup();
            names.len()
        };
        assert_eq!(
            unique_count,
            tools.len(),
            "duplicate tool name in the registry"
        );
    }

    #[test]
    fn initialize_result_names_the_control_plane() {
        let result = initialize_result();
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("control plane")
        );
    }
}
