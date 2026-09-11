//! Shared plan/script builders for `tests/script.rs` — split out purely to keep that file under
//! the workspace's 400-line cap, mirroring `tests/fixture/harness.rs`'s own reason for existing.

use std::collections::{BTreeMap, BTreeSet};

use api2mcp::http::{SsrfPolicy, UrlTemplate};
use api2mcp::model::{
    Access, ApiCall, Budgets, Origin, Pagination, Param, ParamLocation, ParamType, ScriptDef, Slug,
};
use api2mcp::resolve::EndpointPlan;
use api2mcp::resolve::plan::{PlannedApiCall, PlannedTool, ToolTarget};
use api2mcp::runtime::budget::BudgetMeter;
use api2mcp::runtime::dispatch::{AuthProviders, DispatchContext};
use api2mcp::runtime::fanout::ConcurrencyLimits;
use api2mcp::script::{RunScriptError, run_script};

use crate::fixture::Fixture;
use crate::fixture::harness::{service_for, slug};

/// One api_call, `path_template: "/items/{id}"`, reachable by `script_slug` under the alias
/// `"item"` — the shape every test in `tests/script.rs` shares. `id` is a `Path` param so each
/// batch item's own args can steer it to a distinct fixture route, which is what lets a test
/// control per-item timing/content despite `api_many` always naming the same alias.
pub fn plan_with_script(fixture: &Fixture, script_slug: &Slug) -> EndpointPlan {
    let service = service_for(fixture);
    let call = ApiCall {
        slug: slug("item"),
        service_slug: service.slug.clone(),
        auth_provider_slug: None,
        method: ::http::Method::GET,
        path_template: "/items/{id}".to_owned(),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: vec![Param {
            name: "id".to_owned(),
            location: ParamLocation::Path,
            ty: ParamType::String,
            required: true,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        }],
        description: None,
    };
    let planned = PlannedApiCall {
        url_template: UrlTemplate::parse(&call.path_template).expect("valid template"),
        origin: Origin::of(&service.base_url).expect("valid origin"),
        api_call: call.clone(),
        service: service.clone(),
        projection: None,
    };

    let mut calls = BTreeMap::new();
    calls.insert(call.slug.clone(), planned);

    let mut callable = BTreeMap::new();
    callable.insert("item".to_owned(), call.slug.clone());
    let mut callable_by = BTreeMap::new();
    callable_by.insert(script_slug.clone(), callable);

    EndpointPlan {
        slug: slug("ep-script"),
        write_ceiling: Access::Read,
        instructions: None,
        tools: vec![PlannedTool {
            name: "run-script".to_owned(),
            input_schema: serde_json::json!({}),
            target: ToolTarget::Script(script_slug.clone()),
            budgets: Budgets::default(),
        }],
        calls,
        scripts: BTreeMap::new(),
        callable_by,
        origins: BTreeSet::from([Origin::of(&service.base_url).expect("valid origin")]),
        budgets: Budgets::default(),
        digest: "test-digest".to_owned(),
    }
}

pub fn script_def(source: &str) -> ScriptDef {
    ScriptDef {
        slug: slug("demo-script"),
        source: source.to_owned(),
        params: vec![],
        callable: BTreeMap::new(),
        budgets: Budgets::default(),
        description: None,
    }
}

pub async fn run(
    plan: &EndpointPlan,
    budgets: Budgets,
    caller_script: &Slug,
    script: &ScriptDef,
) -> Result<serde_json::Value, RunScriptError> {
    let pool = crate::fixture::harness::loopback_pool();
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let auth = AuthProviders::default();
    let ctx = DispatchContext {
        pool: &pool,
        policy: &policy,
        auth: &auth,
        max_redirects: 5,
    };
    let limits = ConcurrencyLimits::build(plan, budgets.max_concurrency);
    let meter = BudgetMeter::new(budgets);

    run_script(
        plan,
        &ctx,
        &limits,
        &meter,
        caller_script,
        script,
        serde_json::json!({}),
    )
    .await
    .map(|run| run.value)
}
