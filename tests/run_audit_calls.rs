//! Gap 1 of the audit-trail fixes: a request that actually reached the wire must leave a
//! `run_calls` row even when the call as a whole failed — a non-2xx upstream response chief among
//! them (the regression: `DispatchError::HttpStatus` is `Failed`, and `Failed` used to mean "no
//! row at all"). A request that never left (bad arguments) must still leave none.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Param, ParamLocation, ParamType,
    ScriptDef, Service, Slug, Tag, TagExpr,
};
use api2mcp::store::Stores;

use api_support::{admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

async fn seed_service(
    stores: &Stores,
    owner_id: uuid::Uuid,
    name: &str,
    fixture: &Fixture,
) -> Result<Slug> {
    let base_url = fixture.base_url();
    let service = Service {
        owner_id,
        slug: slug(name),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    stores.service().create(&service).await?;
    Ok(service.slug)
}

fn call_with_required_id(owner_id: uuid::Uuid, service_slug: &Slug) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug("thing"),
        service_slug: service_slug.clone(),
        auth_provider_slug: None,
        method: http::Method::GET,
        path_template: "/things/{id}".to_owned(),
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
    }
}

#[tokio::test]
async fn a_404_response_still_writes_a_run_calls_row_and_names_the_failure() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/things/1", Behavior::Status(404));

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-404", &fixture).await?;
    let tag = Tag(slug("tag-404"));
    h.stores
        .api_call()
        .create(
            &call_with_required_id(h.admin_id, &service_slug),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-404"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;

    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/api_calls/thing/test",
        Some(json!({"args": {"id": "1"}, "endpoint": "ep-404"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["status"], json!("error"), "a 404 is a failed call");
    let run_id: uuid::Uuid = body["run_id"].as_str().expect("run_id present").parse()?;

    // This is the regression itself: before the fix, `calls_made: 1` (the whole-batch
    // reservation still committed) came back alongside zero `run_calls` rows — an audit trail
    // that looks complete while quietly recording nothing about the one call that mattered most.
    let (summary, calls) = h
        .stores
        .run()
        .get(h.admin_id, run_id)
        .await?
        .expect("the run was persisted");
    assert_eq!(summary.summary.calls_made, 1);
    assert_eq!(
        calls.len(),
        1,
        "the request reached the wire and got a real response"
    );
    assert_eq!(calls[0].status_code, Some(404));
    assert_eq!(calls[0].seq, 0);

    // The run's own `errors[]` still names the failure — the error rides *alongside* the row,
    // never in place of it.
    assert!(
        summary.errors.as_ref().is_some_and(|e| {
            e.as_array()
                .is_some_and(|a| a.iter().any(|entry| entry["index"] == json!(0)))
        }),
        "errors: {:?}",
        summary.errors
    );

    h.db.teardown().await
}

#[tokio::test]
async fn bad_arguments_fail_before_sending_and_write_no_run_calls_row() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    // Never registered as a route: if this test regresses and a request goes out anyway, the
    // fixture answers 404 rather than letting a silently-passing test hide the bug.

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-bad-args", &fixture).await?;
    let tag = Tag(slug("tag-bad-args"));
    h.stores
        .api_call()
        .create(
            &call_with_required_id(h.admin_id, &service_slug),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-bad-args"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;

    // `id` is required and missing — `schema::bind_args` must reject this before `dispatch` ever
    // reaches `http::bind`/`http::send`.
    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/api_calls/thing/test",
        Some(json!({"args": {}, "endpoint": "ep-bad-args"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["status"], json!("error"));
    let run_id: uuid::Uuid = body["run_id"].as_str().expect("run_id present").parse()?;

    let (summary, calls) = h
        .stores
        .run()
        .get(h.admin_id, run_id)
        .await?
        .expect("the run was persisted");
    // `calls_made` reflects the whole-batch reservation (`BudgetMeter::reserve_calls`), taken
    // before any per-item argument binding — it is *not* "requests that reached the wire", which
    // is exactly what `run_calls` rows are, so it stays 1 even though this item never sent
    // anything. The absence of a row (and of a fixture hit) is the actual assertion here.
    let _ = summary;
    assert!(
        calls.is_empty(),
        "a request that never left must leave no row"
    );
    assert!(
        fixture.seen().is_empty(),
        "no request ever reached the fixture"
    );

    h.db.teardown().await
}

const THREE_CALLS_SCRIPT: &str = r#"
api_many("thing", [#{ "id": "1" }, #{ "id": "2" }, #{ "id": "3" }])
"#;

#[tokio::test]
async fn a_scripts_middle_call_failing_still_records_all_three_rows_in_seq_order() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/things/1", Behavior::Json(json!({"id": "1"})));
    fixture.set("/things/2", Behavior::Status(500));
    fixture.set("/things/3", Behavior::Json(json!({"id": "3"})));

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-three-calls", &fixture).await?;
    let tag = Tag(slug("tag-three-calls"));
    h.stores
        .api_call()
        .create(
            &call_with_required_id(h.admin_id, &service_slug),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .script()
        .create(
            &ScriptDef {
                owner_id: h.admin_id,
                slug: slug("three-calls"),
                source: THREE_CALLS_SCRIPT.to_owned(),
                params: Vec::new(),
                callable: BTreeMap::from([("thing".to_owned(), slug("thing"))]),
                budgets: Budgets::default(),
                description: None,
            },
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-three-calls"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;

    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/scripts/three-calls/test",
        Some(json!({"args": {}, "endpoint": "ep-three-calls"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert!(body["failure"].is_null(), "api_many never throws: {body:?}");

    let calls = body["calls"].as_array().expect("calls array");
    assert_eq!(
        calls.len(),
        3,
        "all three attempts are recorded, not just the two that succeeded"
    );
    // `run_calls.seq` is the *input* index (I7) — never completion order.
    assert_eq!(calls[0]["seq"], json!(0));
    assert_eq!(calls[0]["status_code"], json!(200));
    assert_eq!(calls[1]["seq"], json!(1));
    assert_eq!(calls[1]["status_code"], json!(500));
    assert_eq!(calls[2]["seq"], json!(2));
    assert_eq!(calls[2]["status_code"], json!(200));

    h.db.teardown().await
}
