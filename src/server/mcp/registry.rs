//! `tools/list`'s tool descriptors, built fresh per request from an [`EndpointPlan`] — the
//! *plan* is the cached artifact ([`crate::resolve::PlanCache`]); rendering it into MCP's
//! `{name, description, inputSchema}` shape is cheap enough to redo on every call (a handful of
//! `serde_json::json!` allocations over data the plan already holds), and doing so means this
//! module never has to know anything about caching or invalidation.
//!
//! Two tool names never come from a plan: [`INVOKE_TOOL_NAME`] and [`LIST_TOOLS_NAME`] — synthetic
//! entries every endpoint carries regardless of its tag selection. See [`super::invoke`] for why
//! `invoke` is a first-class tool rather than a fallback wired only into unusual clients.

use serde_json::{Value, json};

use crate::model::Access;
use crate::resolve::EndpointPlan;
use crate::resolve::plan::{PlannedTool, ToolTarget};

pub const INVOKE_TOOL_NAME: &str = "invoke";
pub const LIST_TOOLS_NAME: &str = "list_tools";

/// The full `tools/list` payload: every tool `plan`'s tag selection produced, plus the two
/// synthetic dispatcher tools every endpoint exposes.
pub fn tool_list(plan: &EndpointPlan) -> Value {
    let mut tools: Vec<Value> = plan
        .tools
        .iter()
        .map(|tool| tool_descriptor(plan, tool))
        .collect();
    tools.push(invoke_descriptor());
    tools.push(list_tools_descriptor());
    json!({ "tools": tools })
}

fn tool_descriptor(plan: &EndpointPlan, tool: &PlannedTool) -> Value {
    json!({
        "name": tool.name,
        "description": describe(plan, tool),
        "inputSchema": tool.input_schema,
    })
}

/// The human-authored `description` on an `api_call`/`script` is what a model reads to decide
/// whether and how to call the resulting tool — worth far more than a restatement of the route,
/// so it wins whenever the definer set one. The synthesised (api_call) / generic-label (script)
/// forms below are only the fallback for a definition nobody has documented yet.
fn describe(plan: &EndpointPlan, tool: &PlannedTool) -> String {
    match &tool.target {
        ToolTarget::ApiCall(slug) => plan
            .calls
            .get(slug)
            .map(|planned| {
                planned.api_call.description.clone().unwrap_or_else(|| {
                    let access = match planned.api_call.access {
                        Access::Read => "read",
                        Access::Write => "write",
                    };
                    format!(
                        "{} {} ({access} api call on {})",
                        planned.api_call.method,
                        planned.api_call.path_template,
                        planned.service.slug
                    )
                })
            })
            .unwrap_or_else(|| tool.name.clone()),
        ToolTarget::Script(slug) => plan
            .scripts
            .get(slug)
            .and_then(|s| s.description.clone())
            .unwrap_or_else(|| format!("script: {}", tool.name)),
    }
}

fn invoke_descriptor() -> Value {
    json!({
        "name": INVOKE_TOOL_NAME,
        "description": "Call another tool on this endpoint by name. Prefer this to relying on a \
            tools/list_changed notification, which not every client re-fetches on — call \
            list_tools instead if the tool set might have changed.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool_name": {
                    "type": "string",
                    "description": "Name of the tool to call, exactly as tools/list reports it."
                },
                "args": {
                    "type": "object",
                    "description": "Arguments for that tool, as if calling it directly."
                }
            },
            "required": ["tool_name"]
        }
    })
}

fn list_tools_descriptor() -> Value {
    json!({
        "name": LIST_TOOLS_NAME,
        "description": "Lists every tool this endpoint exposes, with its description and input \
            schema — the same information tools/list returns, callable as an ordinary tool call \
            when a client doesn't re-run tools/list mid-session.",
        "inputSchema": { "type": "object", "properties": {}, "required": [] }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::resolve::plan::{CompiledProjection, PlannedApiCall, PlannedTool, ToolTarget};

    fn planned_tool(name: &str, target: ToolTarget) -> PlannedTool {
        PlannedTool {
            name: name.to_owned(),
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
            target,
            budgets: crate::model::Budgets::default(),
        }
    }

    fn planned_api_call() -> PlannedApiCall {
        PlannedApiCall {
            api_call: crate::model::ApiCall {
                owner_id: uuid::Uuid::nil(),
                slug: "get-item".parse().unwrap(),
                service_slug: "demo".parse().unwrap(),
                auth_provider_slug: None,
                method: http::Method::GET,
                path_template: "/items/{id}".to_owned(),
                query_fixed: BTreeMap::new(),
                body_template: None,
                access: Access::Read,
                idempotent: true,
                projection: None,
                pagination: crate::model::Pagination::None,
                timeout_ms: None,
                max_response_bytes: None,
                params: vec![],
                description: None,
            },
            service: crate::model::Service {
                owner_id: uuid::Uuid::nil(),
                slug: "demo".parse().unwrap(),
                base_url: "https://demo.example.com".parse().unwrap(),
                origin_allowlist: Default::default(),
                default_headers: BTreeMap::new(),
                timeout_ms: 5_000,
                max_concurrency: 4,
                rate_limit_per_min: None,
                max_response_bytes: 1_000_000,
            },
            origin: crate::model::Origin::of(&"https://demo.example.com".parse().unwrap()).unwrap(),
            url_template: crate::http::UrlTemplate::parse("/items/{id}").unwrap(),
            projection: None::<CompiledProjection>,
        }
    }

    fn empty_plan() -> EndpointPlan {
        EndpointPlan {
            owner_id: uuid::Uuid::nil(),
            slug: "ep".parse().unwrap(),
            write_ceiling: Access::Read,
            instructions: None,
            tools: vec![planned_tool(
                "get-item",
                ToolTarget::ApiCall("get-item".parse().unwrap()),
            )],
            calls: BTreeMap::from([("get-item".parse().unwrap(), planned_api_call())]),
            scripts: BTreeMap::new(),
            callable_by: BTreeMap::new(),
            origins: Default::default(),
            budgets: crate::model::Budgets::default(),
            digest: "digest".to_owned(),
        }
    }

    #[test]
    fn tool_list_always_carries_the_two_synthetic_dispatcher_tools() {
        let plan = empty_plan();
        let list = tool_list(&plan);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&INVOKE_TOOL_NAME));
        assert!(names.contains(&LIST_TOOLS_NAME));
        assert!(names.contains(&"get-item"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn an_api_call_tool_gets_a_synthesised_description() {
        let plan = empty_plan();
        let tool = plan.tool("get-item").unwrap();
        let desc = describe(&plan, tool);
        assert!(desc.contains("GET"));
        assert!(desc.contains("/items/{id}"));
        assert!(desc.contains("read"));
    }

    #[test]
    fn an_api_call_tool_uses_the_stored_description_when_present() {
        let mut plan = empty_plan();
        plan.calls
            .get_mut(&"get-item".parse().unwrap())
            .unwrap()
            .api_call
            .description = Some("Fetch one item by id; returns title, url and owner.".to_owned());
        let tool = plan.tool("get-item").unwrap();
        let desc = describe(&plan, tool);
        assert_eq!(desc, "Fetch one item by id; returns title, url and owner.");

        let list = tool_list(&plan);
        let get_item = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "get-item")
            .expect("get-item listed");
        assert_eq!(
            get_item["description"],
            json!("Fetch one item by id; returns title, url and owner.")
        );
    }

    #[test]
    fn a_script_tool_falls_back_to_a_generic_description_when_undocumented() {
        let mut plan = empty_plan();
        let script = crate::model::ScriptDef {
            owner_id: uuid::Uuid::nil(),
            slug: "compose".parse().unwrap(),
            source: "()".to_owned(),
            params: vec![],
            callable: BTreeMap::new(),
            budgets: crate::model::Budgets::default(),
            description: None,
        };
        plan.scripts.insert(script.slug.clone(), script);
        let tool = planned_tool("compose", ToolTarget::Script("compose".parse().unwrap()));
        assert_eq!(describe(&plan, &tool), "script: compose");
    }

    #[test]
    fn invoke_and_list_tools_schemas_are_well_formed() {
        let invoke = invoke_descriptor();
        assert_eq!(invoke["inputSchema"]["required"], json!(["tool_name"]));
        let list = list_tools_descriptor();
        assert_eq!(list["inputSchema"]["type"], json!("object"));
    }
}
