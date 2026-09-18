//! Integration tests for `GET /api/endpoints/{slug}/pack` and `POST /api/packs/import` —
//! pack export/import over the HTTP API (`src/server/api/packs.rs`). Session-only and
//! owner-scoped like every other route in `server::api`; the format is exactly what
//! `api2mcp export`/`api2mcp import` produce and accept (see `pack`'s own module doc).

mod api_support;
mod common;
mod fixture;

use anyhow::Result;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use api_support::{admin, bearer, setup};

async fn seed_service_and_call(h: &api_support::Harness, suffix: &str) {
    let service_body = json!({
        "slug": format!("svc-pack-{suffix}"),
        "base_url": format!("https://svc-pack-{suffix}.example.com/"),
        "origin_allowlist": [format!("https://svc-pack-{suffix}.example.com")],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1_000_000
    });
    let (status, resp) = admin(h, Method::POST, "/api/services", Some(service_body)).await;
    assert_eq!(status, StatusCode::CREATED, "seeding service: {resp:?}");

    let call_body = json!({
        "slug": format!("call-pack-{suffix}"),
        "service": format!("svc-pack-{suffix}"),
        "method": "GET",
        "path_template": "/things",
        "access": "read",
        "tags": [format!("tag-pack-{suffix}")]
    });
    let (status, resp) = admin(h, Method::POST, "/api/api_calls", Some(call_body)).await;
    assert_eq!(status, StatusCode::CREATED, "seeding api_call: {resp:?}");

    let endpoint_body = json!({
        "slug": format!("ep-pack-{suffix}"),
        "tag_expr": format!("has({})", format!("tag-pack-{suffix}"))
    });
    let (status, resp) = admin(h, Method::POST, "/api/endpoints", Some(endpoint_body)).await;
    assert_eq!(status, StatusCode::CREATED, "seeding endpoint: {resp:?}");
}

/// Fetches a raw (non-JSON) response body as text, since `api_support::admin` always tries to
/// decode JSON — the export route deliberately returns `text/yaml` instead.
async fn admin_raw(
    h: &api_support::Harness,
    method: Method,
    path: &str,
) -> (StatusCode, String, Option<String>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, &h.admin_cookie)
        .body(Body::empty())
        .expect("building a valid request");
    let response = h
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router call succeeds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading the response body")
        .to_bytes();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        content_type,
    )
}

async fn admin_yaml_import(
    h: &api_support::Harness,
    path: &str,
    yaml: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::COOKIE, &h.admin_cookie)
        .header(header::CONTENT_TYPE, "text/yaml")
        .body(Body::from(yaml.to_owned()))
        .expect("building a valid request");
    let response = h
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router call succeeds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading the response body")
        .to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
    });
    (status, value)
}

#[tokio::test]
async fn export_returns_yaml_naming_the_endpoints_own_definitions() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service_and_call(&h, "export").await;

    let (status, body, content_type) =
        admin_raw(&h, Method::GET, "/api/endpoints/ep-pack-export/pack").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        content_type.unwrap_or_default().starts_with("text/yaml"),
        "export must be served as text/yaml"
    );

    let pack: api2mcp::pack::Pack = serde_norway::from_str(&body).expect("valid pack yaml");
    assert!(pack.services.contains_key("svc-pack-export"));
    assert!(pack.api_calls.contains_key("call-pack-export"));
    assert!(pack.endpoints.contains_key("ep-pack-export"));
    api2mcp::pack::validate(&pack).expect("exported pack is always valid");

    h.db.teardown().await
}

#[tokio::test]
async fn export_of_an_unknown_endpoint_is_404() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let (status, body, _) = admin_raw(&h, Method::GET, "/api/endpoints/no-such-ep/pack").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
    h.db.teardown().await
}

#[tokio::test]
async fn import_dry_run_classifies_without_writing_anything() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service_and_call(&h, "dryrun").await;
    let (status, body, _) = admin_raw(&h, Method::GET, "/api/endpoints/ep-pack-dryrun/pack").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (status, report) = admin_yaml_import(&h, "/api/packs/import?dry_run=true", &body).await;
    assert_eq!(status, StatusCode::OK, "report: {report:?}");
    assert_eq!(report["dry_run"], json!(true));
    // Everything already exists (it was seeded through the admin API, not this import), so a
    // dry run must classify every row `unchanged`, not `created`.
    for section in ["services", "api_calls", "endpoints"] {
        for row in report[section].as_array().expect("rows array") {
            assert_eq!(
                row["change"],
                json!("unchanged"),
                "section {section}: {row:?}"
            );
        }
    }

    h.db.teardown().await
}

#[tokio::test]
async fn import_is_idempotent_over_http() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service_and_call(&h, "httpidem").await;
    let (status, body, _) =
        admin_raw(&h, Method::GET, "/api/endpoints/ep-pack-httpidem/pack").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Delete everything first so the real import below actually creates rows.
    let (status, _) = admin(&h, Method::DELETE, "/api/endpoints/ep-pack-httpidem", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin(
        &h,
        Method::DELETE,
        "/api/api_calls/call-pack-httpidem",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, first) = admin_yaml_import(&h, "/api/packs/import", &body).await;
    assert_eq!(status, StatusCode::OK, "first import: {first:?}");
    assert_eq!(
        first["api_calls"][0]["change"],
        json!("created"),
        "first import: {first:?}"
    );

    let (status, second) = admin_yaml_import(&h, "/api/packs/import", &body).await;
    assert_eq!(status, StatusCode::OK, "second import: {second:?}");
    assert_eq!(
        second["api_calls"][0]["change"],
        json!("unchanged"),
        "re-importing an unchanged pack must be a no-op: {second:?}"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn invalid_pack_reports_every_failure_at_once() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let yaml = "version: 999\nservices: {}\n";
    let (status, body) = admin_yaml_import(&h, "/api/packs/import", yaml).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:?}");
    let errors = body["errors"].as_array().expect("errors array");
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("999")));

    h.db.teardown().await
}

/// The Change 2 regression test at the HTTP layer: an older pack's top-level `auth_providers:`
/// key must be rejected outright, not silently ignored and half-imported.
#[tokio::test]
async fn a_pack_with_a_top_level_auth_providers_key_is_rejected() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let yaml = "version: 1\nservices: {}\nauth_providers: {}\n";
    let (status, body) = admin_yaml_import(&h, "/api/packs/import", yaml).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:?}");

    h.db.teardown().await
}

#[tokio::test]
async fn a_service_token_cannot_reach_either_pack_route() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service_and_call(&h, "i5").await;

    let (status, resp) = bearer(
        &h,
        Method::GET,
        "/api/endpoints/ep-pack-i5/pack",
        &h.mcp_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {resp:?}");

    let (status, resp) = bearer(
        &h,
        Method::POST,
        "/api/packs/import",
        &h.mcp_token,
        Some(json!({"version": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {resp:?}");

    h.db.teardown().await
}
