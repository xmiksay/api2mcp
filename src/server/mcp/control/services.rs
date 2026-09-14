//! `service.list`/`get`/`create`/`update`/`delete` — an upstream HTTP origin an api_call binds
//! to. Every handler here is a thin adapter over `server::api::services`'s own `_for_owner`
//! functions; see that module's doc and `super`'s module doc for why.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::dto::ServiceCreate;
use crate::server::api::services;
use crate::server::state::AppState;

use super::ToolOutcome;
use super::schema::{slug_schema, with_slug};
use super::support::{SlugArgs, outcome_of, parse_args};

fn pack_service_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "base_url": {
                "type": "string",
                "description": "The service's own base URL, e.g. \"https://api.example.com\"."
            },
            "origin_allowlist": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Origins (scheme+host+port) an api_call on this service may \
                    reach — must include base_url's own origin. A redirect landing outside this \
                    set is refused."
            },
            "default_headers": {
                "type": "object",
                "additionalProperties": {"type": "string"},
                "description": "Headers sent on every request to this service."
            },
            "timeout_ms": {"type": "integer"},
            "max_concurrency": {
                "type": "integer",
                "description": "Max concurrent requests to this service across all callers."
            },
            "rate_limit_per_min": {"type": "integer"},
            "max_response_bytes": {"type": "integer"}
        },
        "required": ["base_url", "timeout_ms", "max_concurrency", "max_response_bytes"]
    })
}

pub(super) fn descriptors() -> Vec<Value> {
    let create_schema = with_slug("Service", pack_service_schema());
    vec![
        json!({
            "name": "service.list",
            "description": "List every service you own.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
        }),
        json!({
            "name": "service.get",
            "description": "Get one service by slug.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("Service")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "service.create",
            "description": "Define a new upstream HTTP service. An api_call always belongs to \
                exactly one service.",
            "inputSchema": create_schema.clone()
        }),
        json!({
            "name": "service.update",
            "description": "Replace an existing service's definition. Effective immediately for \
                every api_call on it.",
            "inputSchema": create_schema
        }),
        json!({
            "name": "service.delete",
            "description": "Delete a service. Refused if any api_call still references it — \
                remove those first.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("Service")},
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
        "service.list" => outcome_of(services::list_for_owner(state, owner_id).await),
        "service.get" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(services::get_by_owner(state, owner_id, &a.slug).await),
            Err(o) => o,
        },
        "service.create" => match parse_args::<ServiceCreate>(args) {
            Ok(a) => outcome_of(services::create_for_owner(state, owner_id, a.slug, a.def).await),
            Err(o) => o,
        },
        "service.update" => match parse_args::<ServiceCreate>(args) {
            Ok(a) => outcome_of(services::update_for_owner(state, owner_id, a.slug, a.def).await),
            Err(o) => o,
        },
        "service.delete" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(
                services::delete_for_owner(state, owner_id, a.slug)
                    .await
                    .map(|()| json!({"deleted": true})),
            ),
            Err(o) => o,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_descriptor_has_a_name_description_and_input_schema() {
        for d in descriptors() {
            assert!(d["name"].as_str().is_some());
            assert!(d["description"].as_str().is_some_and(|s| !s.is_empty()));
            assert_eq!(d["inputSchema"]["type"], json!("object"));
        }
    }
}
