//! `script.list`/`get`/`create`/`update`/`delete`/`test` — a small Rhai program composing one or
//! more api_calls (declared in `callable`) into a single tool, for logic a single HTTP call can't
//! express.

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::dto::ScriptCreate;
use crate::server::api::scripts;
use crate::server::api::test_run::run_script_test;
use crate::server::identity::{Caller, CallerKind};
use crate::server::state::AppState;

use super::ToolOutcome;
use super::schema::{budgets_schema, params_array_schema, slug_schema, tags_schema, with_slug};
use super::support::{SlugArgs, api_error_outcome, outcome_of, parse_args};

fn pack_script_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source": {
                "type": "string",
                "description": "Rhai source. Call another api_call from inside it with \
                    `api(\"alias\", args)` (single call) or `api_many(\"alias\", [args, ...])` \
                    (a fan-out batch) — \"alias\" must be a key of `callable`, below."
            },
            "params": params_array_schema(),
            "callable": {
                "type": "object",
                "additionalProperties": {"type": "string"},
                "description": "alias (as used in the script's own api()/api_many() calls) -> \
                    api_call slug. Only api_calls listed here, and also selected by the exposing \
                    endpoint's tag_expr, are ever reachable from this script."
            },
            "budgets": budgets_schema(),
            "description": {
                "type": "string",
                "description": "What a calling model reads to decide whether and how to call \
                    the resulting tool. Without one, the tool falls back to a generic \
                    \"script: <name>\" label."
            },
            "tags": tags_schema()
        },
        "required": ["source"]
    })
}

#[derive(Deserialize)]
struct TestArgs {
    slug: String,
    endpoint: String,
    #[serde(default)]
    args: Value,
}

pub(super) fn descriptors() -> Vec<Value> {
    let create_schema = with_slug("script", pack_script_schema());
    vec![
        json!({
            "name": "script.list",
            "description": "List every script you own.",
            "inputSchema": {"type": "object", "properties": {}, "required": []}
        }),
        json!({
            "name": "script.get",
            "description": "Get one script by slug.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("script")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "script.create",
            "description": "Define a new Rhai script composing one or more existing api_calls \
                into a single tool. Every api_call it names in `callable` must already exist. \
                Tag it, then reference it from an endpoint's tag_expr to make it callable; test \
                it with script.test first.",
            "inputSchema": create_schema.clone()
        }),
        json!({
            "name": "script.update",
            "description": "Replace an existing script's definition.",
            "inputSchema": create_schema
        }),
        json!({
            "name": "script.delete",
            "description": "Delete a script. Refused if an endpoint alias still references it.",
            "inputSchema": {
                "type": "object",
                "properties": {"slug": slug_schema("script")},
                "required": ["slug"]
            }
        }),
        json!({
            "name": "script.test",
            "description": "Run a script for real, through a named endpoint that exposes it, \
                and return its result plus every upstream call it made along the way (raw \
                response bodies included) or, on failure, a structured error with line/column \
                and a source snippet when the engine has one. Use this before considering any \
                script definition done.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug_schema("script"),
                    "endpoint": {
                        "type": "string",
                        "description": "Slug of an endpoint whose tag_expr already selects this \
                            script."
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments for the script's own model-visible params."
                    }
                },
                "required": ["slug", "endpoint"]
            }
        }),
    ]
}

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
        "script.list" => outcome_of(scripts::list_for_owner(state, owner_id).await),
        "script.get" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(scripts::get_by_owner(state, owner_id, &a.slug).await),
            Err(o) => o,
        },
        "script.create" => match parse_args::<ScriptCreate>(args) {
            Ok(a) => outcome_of(scripts::create_for_owner(state, owner_id, a.slug, a.def).await),
            Err(o) => o,
        },
        "script.update" => match parse_args::<ScriptCreate>(args) {
            Ok(a) => outcome_of(scripts::update_for_owner(state, owner_id, a.slug, a.def).await),
            Err(o) => o,
        },
        "script.delete" => match parse_args::<SlugArgs>(args) {
            Ok(a) => outcome_of(
                scripts::delete_for_owner(state, owner_id, a.slug)
                    .await
                    .map(|()| json!({"deleted": true})),
            ),
            Err(o) => o,
        },
        "script.test" => match parse_args::<TestArgs>(args) {
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
    let Ok(script_slug) = a.slug.parse() else {
        return ToolOutcome::error(format!("invalid script slug {:?}", a.slug));
    };
    let caller = synthetic_caller(owner_id);
    match run_script_test(state, &endpoint_slug, &script_slug, a.args, &caller).await {
        Ok(result) => super::support::ok_value(result),
        Err(err) => api_error_outcome(err),
    }
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
