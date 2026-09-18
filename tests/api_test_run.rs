//! Integration tests for `POST /api/api_calls/{slug}/test` and `POST /api/scripts/{slug}/test` —
//! the routes that make a definition editable: run it for real against `tests/fixture::Fixture`
//! and check raw and projected output side by side, the per-call breakdown, and a failing
//! script's line number.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::model::{
    Access, ApiCall, Budgets, Cardinality, EndpointDef, Origin, Pagination, Param, ParamLocation,
    ParamType, Projection, ProjectionField, ScriptDef, Service, Slug, Tag, TagExpr,
};
use api2mcp::store::Stores;
use uuid::Uuid;

use api_support::{admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

async fn seed_service(
    stores: &Stores,
    owner_id: Uuid,
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

fn get_item_call(owner_id: Uuid, service_slug: &Slug) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug("get-item"),
        service_slug: service_slug.clone(),
        method: http::Method::GET,
        path_template: "/items/{id}".to_owned(),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: Some(Projection {
            fields: vec![
                ProjectionField {
                    name: "id".to_owned(),
                    path: "$.id".to_owned(),
                    cardinality: Cardinality::One,
                    coerce: None,
                },
                ProjectionField {
                    name: "title".to_owned(),
                    path: "$.title".to_owned(),
                    cardinality: Cardinality::One,
                    coerce: None,
                },
            ],
        }),
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
        description: Some("fetch one item".to_owned()),
    }
}

fn list_items_call(owner_id: Uuid, service_slug: &Slug) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug("list-items"),
        service_slug: service_slug.clone(),
        method: http::Method::GET,
        path_template: "/items".to_owned(),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: Vec::new(),
        description: Some("list items".to_owned()),
    }
}

const SUMMARY_SCRIPT_SOURCE: &str = r#"
let listing = api("list", #{});
let items = listing["items"];
if items.len() == 0 {
    #{ "count": 0, "first": () }
} else {
    let first_id = items[0]["id"];
    let detail = api("get", #{ "id": first_id });
    #{ "count": items.len(), "first": detail }
}
"#;

#[tokio::test]
async fn api_call_test_run_returns_raw_and_projected_side_by_side_and_writes_a_run() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set(
        "/items/1",
        Behavior::Json(
            json!({"id": "1", "title": "One", "internal_note": "dropped by projection"}),
        ),
    );

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-test-run", &fixture).await?;
    h.stores
        .api_call()
        .create(
            &get_item_call(h.admin_id, &service_slug),
            &BTreeSet::from([Tag(slug("tr"))]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-test-run"),
            tag_expr: TagExpr::Has(Tag(slug("tr"))),
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
        "/api/api_calls/get-item/test",
        Some(json!({"args": {"id": "1"}, "endpoint": "ep-test-run"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["projected"], json!({"id": "1", "title": "One"}));
    assert_eq!(
        body["raw"],
        json!({"id": "1", "title": "One", "internal_note": "dropped by projection"})
    );
    let run_id = body["run_id"].as_str().expect("run_id present");

    let (summary, calls) = h
        .stores
        .run()
        .get(h.admin_id, run_id.parse()?)
        .await?
        .expect("the test run was persisted");
    assert_eq!(summary.summary.tool_name, "get-item");
    assert_eq!(calls.len(), 1);

    // An api_call not exposed by the endpoint is a 404, not a 500.
    let (status, _) = admin(
        &h,
        Method::POST,
        "/api/api_calls/no-such-call/test",
        Some(json!({"args": {}, "endpoint": "ep-test-run"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}

#[tokio::test]
async fn script_test_run_reports_the_per_call_breakdown_in_input_order() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set(
        "/items",
        Behavior::Json(json!({"items": [{"id": "1", "title": "One"}]})),
    );
    fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-script-run", &fixture).await?;
    let tag = Tag(slug("tr-script"));
    h.stores
        .api_call()
        .create(
            &get_item_call(h.admin_id, &service_slug),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .api_call()
        .create(
            &list_items_call(h.admin_id, &service_slug),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .script()
        .create(
            &ScriptDef {
                owner_id: h.admin_id,
                slug: slug("item-summary"),
                source: SUMMARY_SCRIPT_SOURCE.to_owned(),
                params: Vec::new(),
                callable: BTreeMap::from([
                    ("get".to_owned(), slug("get-item")),
                    ("list".to_owned(), slug("list-items")),
                ]),
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
            slug: slug("ep-script-run"),
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
        "/api/scripts/item-summary/test",
        Some(json!({"args": {}, "endpoint": "ep-script-run"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert!(body["failure"].is_null());
    assert_eq!(body["value"]["count"], json!(1));

    let calls = body["calls"].as_array().expect("calls array");
    assert_eq!(calls.len(), 2, "the script made two upstream calls");
    // Input order — the script calls `list` before `get`.
    assert_eq!(calls[0]["api_call_slug"], json!("list-items"));
    assert_eq!(calls[1]["api_call_slug"], json!("get-item"));
    assert_eq!(calls[0]["status_code"], json!(200));
    assert!(calls[0]["raw"].is_object());

    h.db.teardown().await
}

#[tokio::test]
async fn a_failing_script_test_reports_a_line_number_and_snippet() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    let service_slug = seed_service(&h.stores, h.admin_id, "svc-script-fail", &fixture).await?;
    let tag = Tag(slug("tr-fail"));
    h.stores
        .script()
        .create(
            &ScriptDef {
                owner_id: h.admin_id,
                slug: slug("broken-script"),
                // A deliberate syntax error on an existing source line — never reaches the
                // network, fails to compile — so `snippet_with_caret` has a real line to quote.
                source: "let x = 1;\nlet y = ;".to_owned(),
                params: Vec::new(),
                callable: BTreeMap::new(),
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
            slug: slug("ep-script-fail"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;
    let _ = service_slug;

    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/scripts/broken-script/test",
        Some(json!({"args": {}, "endpoint": "ep-script-fail"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a test-run failure is a 200 with `failure` set: {body:?}"
    );
    assert!(body["value"].is_null());
    let failure = &body["failure"];
    assert_eq!(failure["kind"], json!("compile"));
    assert!(failure["line"].as_u64().is_some(), "failure: {failure:?}");
    assert!(
        failure["snippet"].as_str().is_some_and(|s| s.contains('^')),
        "failure: {failure:?}"
    );

    // The run is still persisted — a test run is a real run, success or not.
    let run_id: uuid::Uuid = body["run_id"].as_str().unwrap().parse()?;
    assert!(h.stores.run().get(h.admin_id, run_id).await?.is_some());

    h.db.teardown().await
}
