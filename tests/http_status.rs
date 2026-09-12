//! A non-2xx upstream response must be a *failure*, not a value.
//!
//! This is the bug these tests exist to prevent regressing: before the status check, a 404 or a
//! 500 had its error body parsed and projected and handed back as a successful result. A model
//! cannot tell that apart from real data — worse, projection usually *succeeds* on it, because an
//! error payload is still JSON. A wrong answer that looks right is the most expensive failure
//! mode this system has.

mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use api2mcp::http::{SsrfPolicy, UrlTemplate};
use api2mcp::model::{Access, ApiCall, Budgets, Origin, Pagination};
use api2mcp::resolve::EndpointPlan;
use api2mcp::resolve::plan::{PlannedApiCall, PlannedTool, ToolTarget};
use api2mcp::runtime::dispatch::{self, AuthProviders, CallBudget, DispatchContext, DispatchError};

use fixture::harness::{loopback_pool, service_for, slug};
use fixture::{Behavior, Fixture};

fn plan_for(fixture: &Fixture, name: &str) -> EndpointPlan {
    let service = service_for(fixture);
    let call = ApiCall {
        owner_id: uuid::Uuid::nil(),
        slug: slug(name),
        service_slug: service.slug.clone(),
        auth_provider_slug: None,
        method: ::http::Method::GET,
        path_template: format!("/{name}"),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: vec![],
        description: None,
    };
    let planned = PlannedApiCall {
        url_template: UrlTemplate::parse(&call.path_template).expect("valid template"),
        origin: Origin::of(&service.base_url).expect("valid origin"),
        api_call: call.clone(),
        service: service.clone(),
        projection: None,
    };
    EndpointPlan {
        owner_id: uuid::Uuid::nil(),
        slug: slug("ep-status"),
        write_ceiling: Access::Read,
        instructions: None,
        tools: vec![PlannedTool {
            name: name.to_owned(),
            input_schema: serde_json::json!({}),
            target: ToolTarget::ApiCall(call.slug.clone()),
            budgets: Budgets::default(),
        }],
        calls: BTreeMap::from([(call.slug.clone(), planned)]),
        scripts: BTreeMap::new(),
        callable_by: BTreeMap::new(),
        origins: BTreeSet::from([Origin::of(&service.base_url).expect("valid origin")]),
        budgets: Budgets::default(),
        digest: "test-digest".to_owned(),
    }
}

async fn dispatch_one(plan: &EndpointPlan, name: &str) -> Result<serde_json::Value, DispatchError> {
    let pool = loopback_pool();
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
    let budget = CallBudget {
        max_response_bytes: 1024 * 1024,
        max_pages: 1,
        deadline: Duration::from_secs(10),
    };
    dispatch::dispatch(plan, &ctx, None, name, &serde_json::json!({}), budget)
        .await
        .map(|o| o.value)
}

#[tokio::test]
async fn a_404_is_an_error_not_a_projected_error_body() {
    let f = Fixture::start().await;
    f.set("/gone", Behavior::Status(404));
    let plan = plan_for(&f, "gone");

    let err = dispatch_one(&plan, "gone")
        .await
        .expect_err("a 404 must not come back as a value");

    match err {
        DispatchError::HttpStatus { status, .. } => assert_eq!(status, 404),
        other => panic!("expected HttpStatus, got {other:?}"),
    }
}

#[tokio::test]
async fn a_500_is_an_error() {
    let f = Fixture::start().await;
    f.set("/boom", Behavior::Status(500));
    let plan = plan_for(&f, "boom");

    let err = dispatch_one(&plan, "boom")
        .await
        .expect_err("500 must fail");
    assert!(matches!(err, DispatchError::HttpStatus { status: 500, .. }));
}

#[tokio::test]
async fn the_error_carries_a_kind_a_script_can_branch_on() {
    let f = Fixture::start().await;
    f.set("/gone", Behavior::Status(404));
    let plan = plan_for(&f, "gone");

    let err = dispatch_one(&plan, "gone").await.expect_err("404 fails");
    let json = serde_json::to_value(&err).expect("DispatchError serializes");
    assert_eq!(json["kind"], serde_json::json!("http_status"));
    assert_eq!(json["status"], serde_json::json!(404));
}

#[tokio::test]
async fn a_200_still_succeeds() {
    let f = Fixture::start().await;
    f.set("/ok", Behavior::Json(serde_json::json!({"n": 1})));
    let plan = plan_for(&f, "ok");

    let value = dispatch_one(&plan, "ok").await.expect("200 succeeds");
    assert_eq!(value, serde_json::json!({"n": 1}));
}
