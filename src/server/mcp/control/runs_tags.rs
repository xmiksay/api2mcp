//! `run.list`/`run.get` — the audit trail, so an agent can see what its own tools actually did —
//! and `tag.list`, the tag vocabulary a `tag_expr` (and an api_call's/script's own `tags`) draws
//! from. Grouped into one file: both are small, read-only, and neither needs the definition
//! CRUD/validation machinery the other resource files reuse from `server::api`.

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::convert::parse_slug;
use crate::server::api::runs::{self, DEFAULT_LIMIT, MAX_LIMIT};
use crate::server::state::AppState;
use crate::store::RunFilter;

use super::ToolOutcome;
use super::support::{api_error_outcome, ok_value, outcome_of, parse_args};

#[derive(Deserialize, Default)]
struct RunListArgs {
    endpoint: Option<String>,
    status: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct RunGetArgs {
    id: Uuid,
}

pub(super) fn descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "run.list",
            "description": "List your own recent tool runs (both real and api_call.test/\
                script.test runs), newest last-page-offset first — the audit trail for what \
                your own tools actually did. Filter by endpoint slug and/or status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "endpoint": {"type": "string", "description": "Filter to one endpoint slug."},
                    "status": {
                        "enum": ["ok", "partial", "error", "denied", "budget_exceeded", "timeout"]
                    },
                    "limit": {
                        "type": "integer",
                        "description": format!(
                            "Max rows returned; default {DEFAULT_LIMIT}, capped at {MAX_LIMIT}."
                        )
                    },
                    "offset": {"type": "integer"}
                },
                "required": []
            }
        }),
        json!({
            "name": "run.get",
            "description": "Full detail for one run by id: the compiled definition it ran from, \
                every upstream call it made (with raw response bodies), and its budget snapshot \
                — everything run.list's summary leaves out.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Run id (a UUID)."}},
                "required": ["id"]
            }
        }),
        json!({
            "name": "tag.list",
            "description": "List every tag name in use — the vocabulary a tag_expr can \
                reference and an api_call's/script's own `tags` can be drawn from.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
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
        "run.list" => match parse_args::<RunListArgs>(args) {
            Ok(a) => list(state, owner_id, a).await,
            Err(o) => o,
        },
        "run.get" => match parse_args::<RunGetArgs>(args) {
            Ok(a) => outcome_of(runs::get_for_owner(state, owner_id, a.id).await),
            Err(o) => o,
        },
        "tag.list" => match state.stores().tag().list().await {
            Ok(tags) => {
                let names: Vec<String> = tags.into_iter().map(|t| t.0.into_string()).collect();
                ok_value(names)
            }
            Err(e) => api_error_outcome(crate::server::error::ApiError::from_store(e)),
        },
        _ => return None,
    })
}

async fn list(state: &AppState, owner_id: Uuid, a: RunListArgs) -> ToolOutcome {
    let endpoint_slug = match a.endpoint.as_deref().map(parse_slug).transpose() {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("invalid endpoint: {e}")),
    };
    let status = match a.status.as_deref().map(runs::parse_status).transpose() {
        Ok(s) => s,
        Err(e) => return api_error_outcome(e),
    };
    let filter = RunFilter {
        endpoint_slug,
        status,
        limit: a.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT),
        offset: a.offset.unwrap_or(0),
    };
    outcome_of(runs::list_for_owner(state, owner_id, &filter).await)
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
