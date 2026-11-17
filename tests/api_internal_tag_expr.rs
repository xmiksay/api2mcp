//! `internal_tag_expr` (#7) through the admin JSON API: it round-trips through
//! `POST/PUT/GET /api/endpoints`, is validated with the same grammar as `tag_expr` before any
//! write, and `GET /api/endpoints/{slug}/plan` reports the api_calls it selected under
//! `internal_calls` rather than `tools`. Skipped when `TEST_DATABASE_URL` is unset.

mod api_support;
mod common;
mod fixture;

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use api_support::{admin, setup};

async fn seed(h: &api_support::Harness) -> Result<()> {
    let service = json!({
        "slug": "svc-internal",
        "base_url": "https://svc-internal.example.com/",
        "origin_allowlist": ["https://svc-internal.example.com"],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1000000
    });
    let (status, resp) = admin(h, Method::POST, "/api/services", Some(service)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    for (slug, tag) in [("call-public", "public"), ("call-raw", "raw")] {
        let call = json!({
            "slug": slug,
            "service": "svc-internal",
            "method": "GET",
            "path_template": "/things",
            "access": "read",
            "tags": [tag]
        });
        let (status, resp) = admin(h, Method::POST, "/api/api_calls", Some(call)).await;
        assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    }
    Ok(())
}

fn tool_names(plan: &Value) -> Vec<&str> {
    plan["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name"))
        .collect()
}

#[tokio::test]
async fn internal_tag_expr_round_trips_and_shows_on_the_plan() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await?;

    let body = json!({
        "slug": "ep-internal",
        "tag_expr": "has(public)",
        "internal_tag_expr": "has(raw)"
    });
    let (status, resp) = admin(&h, Method::POST, "/api/endpoints", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    assert_eq!(resp["internal_tag_expr"], json!("has(raw)"));

    let (status, got) = admin(&h, Method::GET, "/api/endpoints/ep-internal", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["internal_tag_expr"], json!("has(raw)"));

    let (status, plan) = admin(&h, Method::GET, "/api/endpoints/ep-internal/plan", None).await;
    assert_eq!(status, StatusCode::OK, "body: {plan:?}");
    assert_eq!(tool_names(&plan), vec!["call-public"]);
    assert_eq!(plan["internal_calls"], json!(["call-raw"]));

    // Omitting the field on a PUT clears it: the body is the whole definition.
    let (status, resp) = admin(
        &h,
        Method::PUT,
        "/api/endpoints/ep-internal",
        Some(json!({"tag_expr": "has(public)"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {resp:?}");
    assert!(resp.get("internal_tag_expr").is_none(), "body: {resp:?}");
    let (_, plan) = admin(&h, Method::GET, "/api/endpoints/ep-internal/plan", None).await;
    assert_eq!(plan["internal_calls"], json!([]));

    h.db.teardown().await
}

#[tokio::test]
async fn a_malformed_internal_tag_expr_is_rejected_before_any_write() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await?;

    let body = json!({
        "slug": "ep-bad-internal",
        "tag_expr": "has(public)",
        "internal_tag_expr": "has(raw"
    });
    let (status, resp) = admin(&h, Method::POST, "/api/endpoints", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {resp:?}");
    let errors = resp["errors"].to_string();
    assert!(errors.contains("internal_tag_expr"), "body: {resp:?}");

    let (status, _) = admin(&h, Method::GET, "/api/endpoints/ep-bad-internal", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}

#[tokio::test]
async fn a_blank_internal_tag_expr_is_stored_as_none() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await?;

    let body = json!({
        "slug": "ep-blank-internal",
        "tag_expr": "has(public) or has(raw)",
        "internal_tag_expr": "  "
    });
    let (status, resp) = admin(&h, Method::POST, "/api/endpoints", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    assert!(resp.get("internal_tag_expr").is_none(), "body: {resp:?}");
    let (_, plan) = admin(
        &h,
        Method::GET,
        "/api/endpoints/ep-blank-internal/plan",
        None,
    )
    .await;
    assert_eq!(tool_names(&plan), vec!["call-public", "call-raw"]);

    h.db.teardown().await
}

#[tokio::test]
async fn an_alias_of_an_internal_api_call_is_rejected_on_write() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await?;

    let body = json!({
        "slug": "ep-alias-internal",
        "tag_expr": "has(public)",
        "internal_tag_expr": "has(raw)",
        "aliases": {"fetch": {"api_call": "call-raw"}}
    });
    let (status, resp) = admin(&h, Method::POST, "/api/endpoints", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {resp:?}");
    assert!(
        resp["errors"].to_string().contains("alias \\\"fetch\\\""),
        "body: {resp:?}"
    );

    h.db.teardown().await
}
