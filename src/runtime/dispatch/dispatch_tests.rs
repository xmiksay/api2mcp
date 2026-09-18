//! `dispatch.rs`'s unit tests, split out purely to keep that file under the workspace's 400-line
//! cap — see `http/bind.rs`'s identical `#[path = "bind_tests.rs"]` split for precedent.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::http::{CallError, PaginateError, UrlTemplate};
use crate::model::{Access, ApiCall, Budgets, Origin, Pagination, Service};
use crate::resolve::plan::{PlannedTool, ToolTarget};
use crate::runtime::budget::BudgetAxis;

fn service() -> Service {
    let base_url: url::Url = "https://svc.example.com/".parse().unwrap();
    Service {
        owner_id: uuid::Uuid::nil(),
        slug: "svc".parse().unwrap(),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

fn api_call(slug: &str) -> ApiCall {
    ApiCall {
        owner_id: uuid::Uuid::nil(),
        slug: slug.parse().unwrap(),
        service_slug: "svc".parse().unwrap(),
        method: http::Method::GET,
        path_template: "/things".to_owned(),
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
    }
}

fn planned(call: ApiCall, service: Service) -> PlannedApiCall {
    PlannedApiCall {
        url_template: UrlTemplate::parse(&call.path_template).unwrap(),
        origin: Origin::of(&service.base_url).unwrap(),
        api_call: call,
        service,
        projection: None,
    }
}

fn bare_plan() -> EndpointPlan {
    let svc = service();
    let call = api_call("call-a");
    let mut calls = BTreeMap::new();
    calls.insert(call.slug.clone(), planned(call.clone(), svc));

    let tool = PlannedTool {
        name: "call-a".to_owned(),
        input_schema: serde_json::json!({}),
        target: ToolTarget::ApiCall(call.slug.clone()),
        budgets: Budgets::default(),
    };

    EndpointPlan {
        owner_id: uuid::Uuid::nil(),
        slug: "ep".parse().unwrap(),
        write_ceiling: Access::Read,
        instructions: None,
        tools: vec![tool],
        calls,
        scripts: BTreeMap::new(),
        callable_by: BTreeMap::new(),
        origins: BTreeSet::new(),
        budgets: Budgets::default(),
        digest: "deadbeef".to_owned(),
    }
}

#[test]
fn direct_invocation_resolves_against_the_tool_set() {
    let plan = bare_plan();
    let found = resolve(&plan, None, "call-a").expect("resolves");
    assert_eq!(found.api_call.slug.as_str(), "call-a");
}

#[test]
fn direct_invocation_of_an_unknown_name_is_not_declared() {
    let plan = bare_plan();
    let err = resolve(&plan, None, "nope").unwrap_err();
    assert!(matches!(err, DispatchError::NotDeclared { .. }));
}

#[test]
fn script_caller_resolves_only_through_callable_by() {
    let mut plan = bare_plan();
    plan.callable_by.insert(
        "script-a".parse().unwrap(),
        BTreeMap::from([("alias".to_owned(), "call-a".parse().unwrap())]),
    );
    let script_slug: Slug = "script-a".parse().unwrap();

    let found = resolve(&plan, Some(&script_slug), "alias").expect("resolves");
    assert_eq!(found.api_call.slug.as_str(), "call-a");

    // The api_call's own slug is *not* itself a valid alias unless declared as one.
    let err = resolve(&plan, Some(&script_slug), "call-a").unwrap_err();
    assert!(matches!(err, DispatchError::NotDeclared { .. }));
}

#[test]
fn script_not_in_callable_by_at_all_is_not_declared() {
    let plan = bare_plan();
    let script_slug: Slug = "unknown-script".parse().unwrap();
    let err = resolve(&plan, Some(&script_slug), "alias").unwrap_err();
    assert!(matches!(err, DispatchError::NotDeclared { .. }));
}

#[test]
fn a_script_target_cannot_be_dispatched_directly() {
    let mut plan = bare_plan();
    plan.tools.push(PlannedTool {
        name: "script-a".to_owned(),
        input_schema: serde_json::json!({}),
        target: ToolTarget::Script("script-a".parse().unwrap()),
        budgets: Budgets::default(),
    });
    let err = resolve(&plan, None, "script-a").unwrap_err();
    assert!(matches!(err, DispatchError::NotAnApiCall { .. }));
}

/// Change 1, end to end through `AuthProviders::load`: two api_calls on the same service share
/// its one provider with no per-call wiring at all — loading resolves it once (by service slug)
/// and both calls' `dispatch` lookups (`ctx.auth.get(&planned.service.slug)`) find it.
#[tokio::test]
async fn two_api_calls_on_one_service_share_its_provider_with_no_per_call_wiring() {
    let Some(db) = crate::store::test_support::ScratchDb::create()
        .await
        .unwrap()
    else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let owner_id = db.create_user().await.unwrap();
    let stores = crate::store::Stores::new(db.db.clone());

    let base_url: url::Url = "https://svc-shared-provider.example.com/".parse().unwrap();
    let svc = Service {
        owner_id,
        slug: "svc-shared-provider".parse().unwrap(),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    stores.service().create(&svc).await.unwrap();

    let provider = crate::model::AuthProvider {
        owner_id,
        slug: "shared-prov".parse().unwrap(),
        service_slug: svc.slug.clone(),
        kind: crate::model::AuthKind::StaticHeader,
        credential: crate::model::CredentialSource::Env("A2M_CRED_TEST_DISPATCH_SHARED".into()),
        header_name: "Authorization".into(),
        value_template: "Bearer {token}".into(),
        scopes: vec![],
        token_url: None,
        bound_origin: Origin::of(&base_url).unwrap(),
    };
    stores.auth_provider().create(&provider).await.unwrap();

    let call_a = ApiCall {
        service_slug: svc.slug.clone(),
        ..api_call("call-a")
    };
    let call_b = ApiCall {
        service_slug: svc.slug.clone(),
        ..api_call("call-b")
    };
    let mut calls = BTreeMap::new();
    calls.insert(call_a.slug.clone(), planned(call_a.clone(), svc.clone()));
    calls.insert(call_b.slug.clone(), planned(call_b.clone(), svc.clone()));

    let mut plan = bare_plan();
    plan.owner_id = owner_id;
    plan.calls = calls;

    let auth = AuthProviders::load(&stores.auth_provider(), &plan)
        .await
        .unwrap();
    assert_eq!(
        auth.get(&svc.slug).map(|p| p.slug.as_str()),
        Some("shared-prov"),
        "call-a's service should resolve the shared provider"
    );
    // The same lookup, by the same service slug, is exactly what `dispatch` does for call-b —
    // there is no separate per-call entry to miss.
    assert_eq!(
        auth.get(&svc.slug).map(|p| p.slug.as_str()),
        Some("shared-prov"),
        "call-b's service should resolve the identical shared provider"
    );

    db.teardown().await.unwrap();
}

#[test]
fn budget_trip_classification_matches_the_two_axes_that_can_smuggle_in_as_per_item_errors() {
    let page_cap = DispatchError::Send(PaginateError::PageCapExceeded { max: 3 });
    assert_eq!(page_cap.budget_trip(), Some(BudgetAxis::Pages));

    let timeout = DispatchError::Send(PaginateError::Call(CallError::Timeout));
    assert_eq!(timeout.budget_trip(), Some(BudgetAxis::WallClock));

    let ordinary = DispatchError::Send(PaginateError::Call(CallError::Transport {
        message: "connection reset".to_owned(),
    }));
    assert_eq!(ordinary.budget_trip(), None);
}
