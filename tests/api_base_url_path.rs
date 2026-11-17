//! Integration tests for a service `base_url` that carries a path (issue #6): the prefix must reach
//! the upstream on a real test run, and a `base_url` with a query string or fragment must be
//! refused at write time rather than stored and silently ignored.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::model::{
    Access, ApiCall, Pagination, Param, ParamLocation, ParamType, Slug, Tag, TagExpr,
};

use api_support::{admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn service_body(slug: &str, base_url: &str, origin: &str) -> serde_json::Value {
    json!({
        "slug": slug,
        "base_url": base_url,
        "origin_allowlist": [origin],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1_000_000
    })
}

fn get_item_call(owner_id: uuid::Uuid, service_slug: &Slug) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug("get-prefixed-item"),
        service_slug: service_slug.clone(),
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
    }
}

#[tokio::test]
async fn an_api_call_on_a_base_url_with_a_path_requests_the_prefixed_path() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/api/v1/items/7", Behavior::Json(json!({"id": "7"})));

    let origin = fixture.base_url();
    let origin = origin.as_str().trim_end_matches('/');
    let body = service_body("svc-prefixed", &format!("{origin}/api/v1"), origin);
    let (status, resp) = admin(&h, Method::POST, "/api/services", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");

    let tag = Tag(slug("prefixed"));
    h.stores
        .api_call()
        .create(
            &get_item_call(h.admin_id, &slug("svc-prefixed")),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&common::endpoint_def(
            h.admin_id,
            "ep-prefixed",
            TagExpr::Has(tag),
        ))
        .await?;

    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/api_calls/get-prefixed-item/test",
        Some(json!({"args": {"id": "7"}, "endpoint": "ep-prefixed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["status"], json!("ok"), "body: {body:?}");
    assert_eq!(body["raw"], json!({"id": "7"}));

    let seen = fixture.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].raw_path_and_query, "/api/v1/items/7");

    h.db.teardown().await
}

#[tokio::test]
async fn a_base_url_with_a_query_or_fragment_is_refused_at_write_time() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    for (slug, base_url) in [
        ("svc-query", "https://svc-query.example.com/api?key=1"),
        ("svc-fragment", "https://svc-fragment.example.com/api#top"),
    ] {
        let origin = format!("https://{slug}.example.com");
        let body = service_body(slug, base_url, &origin);
        let (status, resp) = admin(&h, Method::POST, "/api/services", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{base_url}: {resp:?}");
        assert!(
            resp.to_string().contains("query string or fragment"),
            "{base_url}: {resp:?}"
        );
    }

    h.db.teardown().await
}
