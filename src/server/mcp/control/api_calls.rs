//! `api_call.list`/`get`/`create`/`update`/`delete`/`test` — one HTTP request template against a
//! service; the definition that actually becomes an MCP tool once an endpoint's `tag_expr`
//! selects it.
//!
//! **`auth_provider` never appears here** — not in the `inputSchema`, and not honored even if a
//! caller sends it anyway. See `super`'s module doc for why and what this means for a freshly
//! created api_call's first call.

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::api_calls;
use crate::server::api::dto::ApiCallCreate;
use crate::server::api::test_run::run_api_call_test;
use crate::server::identity::{Caller, CallerKind};
use crate::server::state::AppState;

use super::ToolOutcome;
use super::schema::{
    pagination_schema, params_array_schema, projection_schema, slug_schema, tags_schema, with_slug,
};
use super::support::{SlugArgs, api_error_outcome, outcome_of, parse_args};

fn pack_api_call_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "service": slug_schema("The owning service's"),
            "method": {
                "type": "string",
                "description": "HTTP method, e.g. \"GET\", \"POST\"."
            },
            "path_template": {
                "type": "string",
                "description": "URL path, relative to the service's base_url. \"{name}\" \
                    placeholders bind to this api_call's own `path`-location params, e.g. \
                    \"/items/{id}\"."
            },
            "query_fixed": {
                "type": "object",
                "additionalProperties": {"type": "string"},
                "description": "Query parameters sent on every call, not caller-controlled."
            },
            "body_template": {
                "description": "A JSON request body template; `body`-location params splice \
                    into it by JSON pointer."
            },
            "access": {
                "enum": ["read", "write"],
                "description": "Must be at or below the exposing endpoint's own write_ceiling."
            },
            "idempotent": {"type": "boolean", "default": false},
            "projection": projection_schema(),
            "pagination": pagination_schema(),
            "timeout_ms": {"type": "integer"},
            "max_response_bytes": {"type": "integer"},
            "params": params_array_schema(),
            "tags": tags_schema(),
            "description": {
                "type": "string",
                "description": "What a calling model reads to decide whether and how to call \
                    the resulting tool. Worth writing carefully — without one, the tool's \
                    description falls back to a bare \"METHOD path (access on service)\"."
            }
        },
        "required": ["service", "method", "path_template", "access"]
    })
}

/// The subset of `api_call.test`'s params `parse_args` actually needs — a plain `{slug,
/// endpoint, args}`, not the full create/update shape.
#[derive(Deserialize)]
struct TestArgs {
    slug: String,
    endpoint: String,
    #[serde(default)]
    args: Value,
}

pub(super) fn descriptors() -> Vec<Value> {
    let create_schema = with_slug("api_call", pack_api_call_schema());
    vec![
        json!({
            "name": "api_call.list",
            "description": "List every api_call you own.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
        }),
        json!({
            "name": "api_call.get",
            "description": "Get one api_call by slug.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("api_call")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "api_call.create",
            "description": "Define a new HTTP request template against a service. Has no \
                credential attached and cannot get one through this tool — see this endpoint's \
                own initialize instructions for why. Tag it, then reference it from an \
                endpoint's tag_expr to make it callable; test it with api_call.test first.",
            "inputSchema": create_schema.clone()
        }),
        json!({
            "name": "api_call.update",
            "description": "Replace an existing api_call's definition (cannot move it to a \
                different service — delete and recreate instead). Any credential a human \
                already attached is preserved untouched; this tool can neither see nor change \
                it.",
            "inputSchema": create_schema
        }),
        json!({
            "name": "api_call.delete",
            "description": "Delete an api_call. Refused if a script or an endpoint alias still \
                references it.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("api_call")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "api_call.test",
            "description": "Run an api_call for real against its live upstream, through a named \
                endpoint that exposes it, and return both the raw upstream response and (if the \
                api_call has a projection) the projected shape a caller would actually see. Use \
                this before considering any api_call definition done, and again after any change \
                to its params, projection or service.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug_schema("api_call"),
                    "endpoint": {
                        "type": "string",
                        "description": "Slug of an endpoint whose tag_expr already selects this \
                            api_call."
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments for the api_call's own model-visible params, \
                            as tools/call would send them."
                    }
                },
                "required": ["slug", "endpoint"]
            }
        }),
    ]
}

/// A synthetic [`Caller`] for the audit trail: a control-plane tool call is always made by a
/// `control_plane`-capable service token, so `CallerKind::ServiceToken` is the accurate kind —
/// there is no session or OAuth grant behind this call to attribute it to instead.
fn synthetic_caller(owner_id: Uuid) -> Caller {
    Caller {
        kind: CallerKind::ServiceToken,
        id: owner_id,
    }
}

pub(super) async fn dispatch(
    state: &AppState,
    owner_id: Uuid,
    name: &str,
    args: Value,
) -> Option<ToolOutcome> {
    Some(match name {
        "api_call.list" => outcome_of(api_calls::list_for_owner(state, owner_id).await),
        "api_call.get" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(api_calls::get_by_owner(state, owner_id, &a.slug).await),
            Err(o) => o,
        },
        "api_call.create" => match parse_args::<ApiCallCreate>(args) {
            Ok(mut a) => {
                // I5: an agent can never attach a credential, however it was asked to — a
                // freshly created api_call always starts with none, full stop.
                a.def.auth_provider = None;
                outcome_of(api_calls::create_for_owner(state, owner_id, a.slug, a.def).await)
            }
            Err(o) => o,
        },
        "api_call.update" => match parse_args::<ApiCallCreate>(args) {
            Ok(mut a) => match api_calls::find(state, owner_id, &a.slug).await {
                // I5: whatever a human already bound stays exactly as it was — this tool can
                // neither read nor overwrite it, so the incoming value (if any) is discarded in
                // favor of what is already on the row.
                Ok(existing) => {
                    a.def.auth_provider = existing
                        .api_call
                        .auth_provider_slug
                        .as_ref()
                        .map(|s| s.as_str().to_owned());
                    outcome_of(api_calls::update_for_owner(state, owner_id, a.slug, a.def).await)
                }
                Err(err) => api_error_outcome(err),
            },
            Err(o) => o,
        },
        "api_call.delete" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(
                api_calls::delete_for_owner(state, owner_id, a.slug)
                    .await
                    .map(|()| json!({"deleted": true})),
            ),
            Err(o) => o,
        },
        "api_call.test" => match parse_args::<TestArgs>(args) {
            Ok(a) => test(state, owner_id, a).await,
            Err(o) => o,
        },
        _ => return None,
    })
}

async fn test(state: &AppState, owner_id: Uuid, a: TestArgs) -> ToolOutcome {
    let Ok(endpoint_slug) = a.endpoint.parse() else {
        return ToolOutcome::error(format!("invalid endpoint slug {:?}", a.endpoint));
    };
    let Ok(api_call_slug) = a.slug.parse() else {
        return ToolOutcome::error(format!("invalid api_call slug {:?}", a.slug));
    };
    let caller = synthetic_caller(owner_id);
    match run_api_call_test(state, &endpoint_slug, &api_call_slug, a.args, &caller).await {
        Ok(result) => super::support::ok_value(result),
        Err(err) => api_error_outcome(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_descriptor_mentions_auth_provider() {
        for d in descriptors() {
            let text = d.to_string();
            assert!(
                !text.contains("auth_provider"),
                "tool {:?} must never expose auth_provider: {text}",
                d["name"]
            );
        }
    }

    #[test]
    fn every_descriptor_has_a_name_description_and_input_schema() {
        for d in descriptors() {
            assert!(d["name"].as_str().is_some());
            assert!(d["description"].as_str().is_some_and(|s| !s.is_empty()));
            assert_eq!(d["inputSchema"]["type"], json!("object"));
        }
    }
}
