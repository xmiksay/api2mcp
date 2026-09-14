//! `endpoint.list`/`get`/`create`/`update`/`delete`/`plan` — a `tag_expr` selecting which
//! api_calls/scripts are exposed as tools at `/mcp/{slug}`, plus a write ceiling and a budget.
//!
//! **`auth_providers` never appears here** — an endpoint's own I5 join (`endpoint_auth_providers`,
//! `store::endpoint`'s own doc: "which auth providers this endpoint may bind to"), hidden for the
//! identical reason an api_call's own `auth_provider` is (see `super`'s module doc and
//! `super::api_calls`): a human sets it, never this tool. A freshly created endpoint gets the
//! permissive empty default (every provider a selected api_call is otherwise allowed to bind);
//! `endpoint.update` always preserves whatever is already on the row.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::dto::EndpointCreate;
use crate::server::api::endpoints;
use crate::server::state::AppState;

use super::ToolOutcome;
use super::schema::{budgets_schema, slug_schema, with_slug};
use super::support::{SlugArgs, api_error_outcome, outcome_of, parse_args};

fn pack_endpoint_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tag_expr": {
                "type": "string",
                "description": "Boolean expression over tags selecting which api_calls/scripts \
                    this endpoint exposes, e.g. \"has(read) and not has(deprecated)\". Grammar: \
                    has(tag) | not EXPR | EXPR and EXPR | EXPR or EXPR | (EXPR)."
            },
            "write_ceiling": {
                "enum": ["read", "write"],
                "default": "read",
                "description": "The highest access any selected api_call/script may declare; a \
                    selected api_call above this is a validation error, not a silent exclusion."
            },
            "budgets": budgets_schema(),
            "instructions": {
                "type": "string",
                "description": "Endpoint-specific addendum appended to the generic instructions \
                    a consuming agent sees on tools/list."
            },
            "enabled": {"type": "boolean", "default": true},
            "aliases": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "description": "{\"api_call\": \"<slug>\"} or {\"script\": \"<slug>\"} — \
                        renames that tool's exposed name to the alias key instead of its own \
                        slug."
                },
                "description": "Exposed tool name -> target. Omit for every selected item to \
                    keep its own slug as its tool name."
            }
        },
        "required": ["tag_expr"]
    })
}

pub(super) fn descriptors() -> Vec<Value> {
    let create_schema = with_slug("endpoint", pack_endpoint_schema());
    vec![
        json!({
            "name": "endpoint.list",
            "description": "List every endpoint you own.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
        }),
        json!({
            "name": "endpoint.get",
            "description": "Get one endpoint's own stored definition by slug — its tag_expr, \
                budgets and aliases as written, not the resolved tool list (use endpoint.plan \
                for that).",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("Endpoint")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "endpoint.create",
            "description": "Define a new endpoint: a tag_expr selecting which api_calls/scripts \
                become tools at /mcp/{slug}. Takes effect immediately — there is no draft or \
                publish step. Check it with endpoint.plan afterwards to see the resolved tool \
                list and reachable origins.",
            "inputSchema": create_schema.clone()
        }),
        json!({
            "name": "endpoint.update",
            "description": "Replace an existing endpoint's definition. Takes effect immediately \
                for every consumer of /mcp/{slug}.",
            "inputSchema": create_schema
        }),
        json!({
            "name": "endpoint.delete",
            "description": "Delete an endpoint. Cascades its own aliases and any service-token \
                grant naming it; other definitions it selected are untouched.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("Endpoint")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "endpoint.plan",
            "description": "Resolve an endpoint: the exact tool list a consumer of /mcp/{slug} \
                would see (with generated inputSchemas), plus the statically computed set of \
                origins any of its tools can reach. Use this to check a tag_expr actually \
                selects what you meant, and to see the whole reachable-origin surface at a \
                glance.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("Endpoint")},
                "required": ["slug"]
            }
        }),
    ]
}

pub(super) async fn dispatch(
    state: &AppState,
    owner_id: Uuid,
    name: &str,
    args: Value,
) -> Option<ToolOutcome> {
    Some(match name {
        "endpoint.list" => outcome_of(endpoints::list_for_owner(state, owner_id).await),
        "endpoint.get" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(endpoints::get_by_owner(state, owner_id, &a.slug).await),
            Err(o) => o,
        },
        "endpoint.create" => match parse_args::<EndpointCreate>(args) {
            Ok(mut a) => {
                // I5: the permissive empty default, never a caller-supplied set — see this
                // module's own doc.
                a.def.auth_providers = BTreeSet::new();
                outcome_of(endpoints::create_for_owner(state, owner_id, a.slug, a.def).await)
            }
            Err(o) => o,
        },
        "endpoint.update" => match parse_args::<EndpointCreate>(args) {
            Ok(mut a) => match endpoints::get_by_owner(state, owner_id, &a.slug).await {
                // I5: whatever is already on the row stays exactly as it was.
                Ok(existing) => {
                    a.def.auth_providers = existing.def.auth_providers.clone();
                    outcome_of(endpoints::update_for_owner(state, owner_id, a.slug, a.def).await)
                }
                Err(err) => api_error_outcome(err),
            },
            Err(o) => o,
        },
        "endpoint.delete" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(
                endpoints::delete_for_owner(state, owner_id, a.slug)
                    .await
                    .map(|()| json!({"deleted": true})),
            ),
            Err(o) => o,
        },
        "endpoint.plan" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(endpoints::plan_for_owner(state, owner_id, &a.slug).await),
            Err(o) => o,
        },
        _ => return None,
    })
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
                "tool {:?} must never expose auth_providers: {text}",
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
