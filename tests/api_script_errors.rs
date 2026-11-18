//! Integration tests for issue #5: a failed `api()` call reaches the script as the typed
//! `#{kind, message, status}` error object (not a bare "api call failed"), and a call refused
//! before any HTTP request still shows up in `POST /api/scripts/{slug}/test`'s `calls`.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Param, ParamLocation, ParamType,
    ScriptDef, Service, Slug, Tag, TagExpr,
};

use api_support::{Harness, admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

/// One service, one `get-item` api_call (`GET /items/{id}`), one script calling it as `get`,
/// one endpoint exposing both. `budgets` is the script's own.
async fn seed(h: &Harness, fixture: &Fixture, source: &str, budgets: Budgets) -> Result<()> {
    let owner_id: Uuid = h.admin_id;
    let base_url = fixture.base_url();
    let service = Service {
        owner_id,
        slug: slug("svc"),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    h.stores.service().create(&service).await?;

    let tag = Tag(slug("errs"));
    let call = ApiCall {
        owner_id,
        slug: slug("get-item"),
        service_slug: service.slug.clone(),
        method: http::Method::GET,
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
    h.stores
        .api_call()
        .create(&call, &BTreeSet::from([tag.clone()]))
        .await?;
    h.stores
        .script()
        .create(
            &ScriptDef {
                owner_id,
                slug: slug("probe"),
                source: source.to_owned(),
                params: Vec::new(),
                callable: BTreeMap::from([("get".to_owned(), slug("get-item"))]),
                budgets,
                description: None,
            },
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id,
            slug: slug("ep"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;
    Ok(())
}

async fn run_probe(h: &Harness, include_raw: Value) -> Value {
    let (status, body) = admin(
        h,
        Method::POST,
        "/api/scripts/probe/test",
        Some(json!({"args": {}, "endpoint": "ep", "include_raw": include_raw})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    body
}

/// The issue's own repro: an upstream 401, caught in the script.
#[tokio::test]
async fn a_caught_api_error_is_the_typed_object_with_kind_and_status() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/items/1", Behavior::Status(401));
    fixture.set("/items/2", Behavior::Status(429));
    let source = r#"
        let unauthorized = ();
        try { api("get", #{ "id": "1" }); } catch (e) { unauthorized = e; }
        let limited = api_try("get", #{ "id": "2" });
        #{
            "unauthorized": unauthorized,
            "limited": limited.error,
            "branch": if unauthorized.status == 401 { "unauthorized" } else { "other" },
        }
    "#;
    seed(&h, &fixture, source, Budgets::default()).await?;

    let body = run_probe(&h, json!(false)).await;
    assert!(body["failure"].is_null(), "body: {body:?}");
    let value = &body["value"];
    let unauthorized = &value["unauthorized"];
    assert_eq!(unauthorized["kind"], json!("http_status"));
    assert_eq!(unauthorized["status"], json!(401));
    assert!(
        unauthorized["message"]
            .as_str()
            .is_some_and(|m| m.contains("401")),
        "{unauthorized:?}"
    );
    assert_eq!(value["branch"], json!("unauthorized"));
    // `api_try` carries the identical shape — a script tells "rate limited" apart by status.
    assert_eq!(value["limited"]["kind"], json!("http_status"));
    assert_eq!(value["limited"]["status"], json!(429));

    let calls = body["calls"].as_array().expect("calls array");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["status_code"], json!(401));
    assert_eq!(calls[0]["error"]["kind"], json!("http_status"));
    assert_eq!(calls[0]["error"]["status"], json!(401));
    assert_eq!(calls[1]["error"]["status"], json!(429));

    h.db.teardown().await
}

/// Argument validation and the run's call budget both refuse a call before any HTTP request is
/// made; both must still be visible in `calls`, with the typed error and no status/response.
#[tokio::test]
async fn calls_refused_before_any_request_are_listed_with_their_error() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/items/1", Behavior::Json(json!({"id": "1"})));
    let source = r#"
        let bad_args = ();
        try { api("get", #{}); } catch (e) { bad_args = e; }
        api("get", #{ "id": "1" });
        let over_budget = ();
        try { api("get", #{ "id": "1" }); } catch (e) { over_budget = e; }
        #{
            "bad_args": bad_args,
            "over_budget": over_budget,
            // `status` is always present, so comparing it never throws on a non-HTTP failure.
            "is_404": over_budget.status == 404,
        }
    "#;
    let budgets = Budgets {
        max_calls: Some(2),
        ..Budgets::default()
    };
    seed(&h, &fixture, source, budgets).await?;

    // `include_raw: null` is accepted and means false.
    let body = run_probe(&h, Value::Null).await;
    assert!(body["failure"].is_null(), "body: {body:?}");
    let value = &body["value"];
    assert_eq!(value["bad_args"]["kind"], json!("args"));
    assert_eq!(value["bad_args"]["status"], Value::Null);
    assert_eq!(value["over_budget"]["kind"], json!("budget_exceeded"));
    assert_eq!(value["over_budget"]["axis"], json!("calls"));
    assert_eq!(value["over_budget"]["status"], Value::Null);
    assert_eq!(value["is_404"], json!(false));

    let calls = body["calls"].as_array().expect("calls array");
    assert_eq!(calls.len(), 3, "calls: {calls:?}");
    for (i, kind) in [(0, "args"), (2, "budget_exceeded")] {
        let c = &calls[i];
        assert_eq!(c["seq"], json!(i), "{c:?}");
        assert_eq!(c["api_call_slug"], json!("get-item"));
        assert_eq!(c["service_slug"], json!("svc"));
        assert_eq!(c["error"]["kind"], json!(kind));
        assert!(c["status_code"].is_null(), "{c:?}");
        assert!(c.get("raw").is_none(), "raw is absent by default: {c:?}");
        assert!(c["response_bytes"].is_null());
    }
    assert_eq!(calls[1]["status_code"], json!(200));
    assert!(calls[1]["error"].is_null());
    assert_eq!(
        fixture.seen().len(),
        1,
        "only the valid in-budget call reached HTTP"
    );

    // With include_raw, a refused call's `raw` is an explicit `null`, not absent.
    let body = run_probe(&h, json!(true)).await;
    let calls = body["calls"].as_array().expect("calls array");
    for i in [0, 2] {
        assert_eq!(calls[i].get("raw"), Some(&Value::Null), "{:?}", calls[i]);
    }
    assert_eq!(calls[1]["raw"], json!({"id": "1"}));

    h.db.teardown().await
}

/// An uncaught `api()` error fails the script, but the call that caused it is still listed and
/// the failure message names the typed error rather than "api call failed".
#[tokio::test]
async fn an_uncaught_api_error_keeps_its_call_and_names_the_kind() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/items/1", Behavior::Status(404));
    seed(
        &h,
        &fixture,
        r#"api("get", #{ "id": "1" })"#,
        Budgets::default(),
    )
    .await?;

    let body = run_probe(&h, json!(false)).await;
    let failure = &body["failure"];
    assert_eq!(failure["kind"], json!("runtime"));
    let message = failure["message"].as_str().expect("message");
    assert!(message.contains("http_status"), "{message}");
    assert!(!message.contains("api call failed"), "{message}");

    let calls = body["calls"].as_array().expect("calls array");
    assert_eq!(calls.len(), 1, "body: {body:?}");
    assert_eq!(calls[0]["status_code"], json!(404));
    assert_eq!(calls[0]["error"]["status"], json!(404));

    h.db.teardown().await
}
