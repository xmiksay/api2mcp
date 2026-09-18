//! Integration tests for the read routes of chunk C14's admin JSON API — `GET /api/health`,
//! `/api/me`, `/api/tags`, `/api/runs[/{id}]`, `/api/endpoints/{slug}/plan` — plus the structural
//! authentication check every route in `server::api` shares: no route accepts a bearer service
//! token because [`api2mcp::server::identity::Caller`] only ever extracts from a session cookie.
//! CRUD write flows live in `tests/api_write.rs`; the test-run routes in `tests/api_test_run.rs`.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::model::{Access, ApiCall, EndpointDef, Origin, Pagination, Service, Tag, TagExpr};
use api2mcp::store::Stores;
use uuid::Uuid;

use api_support::{admin, anonymous, bearer, setup};

fn slug(s: &str) -> api2mcp::model::Slug {
    s.parse().expect("valid slug")
}

/// Seeds one service, one tagged api_call, and one endpoint selecting it, all owned by
/// `owner_id` — enough for `GET .../plan` to have something real to resolve.
async fn seed_minimal(stores: &Stores, owner_id: Uuid) -> Result<()> {
    let base_url: url::Url = "https://svc-api-read.example.com/".parse()?;
    let service = Service {
        owner_id,
        slug: slug("svc-api-read"),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    stores.service().create(&service).await?;

    let call = ApiCall {
        owner_id,
        slug: slug("call-api-read"),
        service_slug: service.slug.clone(),
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
        params: Vec::new(),
        description: Some("read things".to_owned()),
    };
    stores
        .api_call()
        .create(&call, &BTreeSet::from([Tag(slug("read-tag"))]))
        .await?;

    let endpoint = EndpointDef {
        owner_id,
        slug: slug("ep-api-read"),
        tag_expr: TagExpr::Has(Tag(slug("read-tag"))),
        write_ceiling: Access::Read,
        budgets: Default::default(),
        instructions: Some("read-only demo endpoint".to_owned()),
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    };
    stores.endpoint().create(&endpoint).await?;
    Ok(())
}

#[tokio::test]
async fn health_reports_version_and_db_connectivity() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let (status, body) = admin(&h, Method::GET, "/api/health", None).await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert!(body["db_connected"].as_bool().unwrap());
    assert!(body["migrations_total"].as_u64().unwrap() > 0);
    assert!(!body["version"].as_str().unwrap().is_empty());

    h.db.teardown().await
}

#[tokio::test]
async fn me_reports_the_calling_session() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let (status, body) = admin(&h, Method::GET, "/api/me", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kind"], json!("session"));
    // No `is_admin` any more (Decision 1): a session reaching this route is already, by
    // construction, a caller who can read and write every definition.
    assert!(body.get("is_admin").is_none());

    h.db.teardown().await
}

#[tokio::test]
async fn no_route_in_this_module_accepts_a_bearer_token_or_no_credential() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    for path in ["/api/health", "/api/me", "/api/services", "/api/tags"] {
        let (status, _) = anonymous(&h, Method::GET, path).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous GET {path}");

        let (status, _) = bearer(&h, Method::GET, path, &h.mcp_token, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "a service token GET {path} must never construct a Caller"
        );
    }

    h.db.teardown().await
}

#[tokio::test]
async fn list_tags_reflects_seeded_tags() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_minimal(&h.stores, h.admin_id).await?;

    let (status, body) = admin(&h, Method::GET, "/api/tags", None).await;
    assert_eq!(status, StatusCode::OK);
    let tags: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(tags.contains(&"read-tag"));

    h.db.teardown().await
}

#[tokio::test]
async fn endpoint_plan_exposes_tools_and_the_reachable_origin_set() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_minimal(&h.stores, h.admin_id).await?;

    let (status, body) = admin(&h, Method::GET, "/api/endpoints/ep-api-read/plan", None).await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    let tools = body["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|t| t["name"] == json!("call-api-read")));
    let origins = body["origins"].as_array().expect("origins array");
    assert!(
        origins
            .iter()
            .any(|o| o == "https://svc-api-read.example.com")
    );
    assert_eq!(body["write_ceiling"], json!("read"));

    // A slug that doesn't exist is a 404, not a 500 or an empty plan.
    let (status, _) = admin(
        &h,
        Method::GET,
        "/api/endpoints/no-such-endpoint/plan",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}

#[tokio::test]
async fn runs_list_is_empty_before_anything_ran_and_a_missing_run_id_is_404() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_minimal(&h.stores, h.admin_id).await?;

    let (status, body) = admin(&h, Method::GET, "/api/runs?endpoint=ep-api-read", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);

    let random_id = uuid::Uuid::new_v4();
    let (status, _) = admin(&h, Method::GET, &format!("/api/runs/{random_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}
