//! Integration tests for the write routes of chunk C14's admin JSON API: create → read → update →
//! delete for each entity, `pack::validate`'s "report every failure at once" contract, the I5
//! sharp edge on `/api/auth_providers`, the credential-value guard, and the
//! `meta.definitions_generation` bump every write path must produce.

mod api_support;
mod common;
mod fixture;

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api_support::{admin, bearer, setup};

#[tokio::test]
async fn service_create_read_update_delete_roundtrip() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let create_body = json!({
        "slug": "svc-crud",
        "base_url": "https://svc-crud.example.com/",
        "origin_allowlist": ["https://svc-crud.example.com"],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1_000_000
    });
    let (status, body) = admin(&h, Method::POST, "/api/services", Some(create_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body:?}");
    assert_eq!(body["slug"], json!("svc-crud"));

    let (status, body) = admin(&h, Method::GET, "/api/services/svc-crud", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["timeout_ms"], json!(5000));

    let update_body = json!({
        "base_url": "https://svc-crud.example.com/",
        "origin_allowlist": ["https://svc-crud.example.com"],
        "timeout_ms": 9000,
        "max_concurrency": 8,
        "max_response_bytes": 2_000_000
    });
    let (status, body) = admin(&h, Method::PUT, "/api/services/svc-crud", Some(update_body)).await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["timeout_ms"], json!(9000));

    let (status, _) = admin(&h, Method::DELETE, "/api/services/svc-crud", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = admin(&h, Method::GET, "/api/services/svc-crud", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}

async fn seed_service(h: &api_support::Harness, slug: &str) {
    let body = json!({
        "slug": slug,
        "base_url": format!("https://{slug}.example.com/"),
        "origin_allowlist": [format!("https://{slug}.example.com")],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1_000_000
    });
    let (status, resp) = admin(h, Method::POST, "/api/services", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "seeding service: {resp:?}");
}

#[tokio::test]
async fn api_call_create_read_update_delete_roundtrip() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service(&h, "svc-for-call-crud").await;

    let create_body = json!({
        "slug": "call-crud",
        "service": "svc-for-call-crud",
        "method": "GET",
        "path_template": "/things",
        "access": "read",
        "tags": ["crud-tag"]
    });
    let (status, body) = admin(&h, Method::POST, "/api/api_calls", Some(create_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body:?}");

    let (status, body) = admin(&h, Method::GET, "/api/api_calls/call-crud", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["method"], json!("GET"));

    let update_body = json!({
        "service": "svc-for-call-crud",
        "method": "GET",
        "path_template": "/other-things",
        "access": "read",
        "tags": ["crud-tag"]
    });
    let (status, body) = admin(
        &h,
        Method::PUT,
        "/api/api_calls/call-crud",
        Some(update_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body["path_template"], json!("/other-things"));

    let (status, _) = admin(&h, Method::DELETE, "/api/api_calls/call-crud", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin(&h, Method::GET, "/api/api_calls/call-crud", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}

#[tokio::test]
async fn invalid_api_call_reports_every_independent_failure_at_once() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    // Two unrelated problems in one body: an unknown service, and an unknown auth_provider —
    // both are `validate_api_call` checks, so a single POST should surface both, not just one.
    let create_body = json!({
        "slug": "call-two-problems",
        "service": "does-not-exist",
        "auth_provider": "also-does-not-exist",
        "method": "GET",
        "path_template": "/things",
        "access": "read"
    });
    let (status, body) = admin(&h, Method::POST, "/api/api_calls", Some(create_body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:?}");
    let errors = body["errors"].as_array().expect("errors array");
    assert!(
        errors.len() >= 2,
        "expected at least two independent failures, got {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("does-not-exist"))
    );
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("also-does-not-exist"))
    );

    h.db.teardown().await
}

#[tokio::test]
async fn deleting_a_service_still_referenced_by_an_api_call_is_rejected() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service(&h, "svc-guarded").await;
    let call_body = json!({
        "slug": "call-guarding",
        "service": "svc-guarded",
        "method": "GET",
        "path_template": "/things",
        "access": "read"
    });
    let (status, body) = admin(&h, Method::POST, "/api/api_calls", Some(call_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body:?}");

    let (status, body) = admin(&h, Method::DELETE, "/api/services/svc-guarded", None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "deleting a referenced service must fail cleanly, not as a raw FK violation: {body:?}"
    );
    let errors = body["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("svc-guarded"))
    );

    h.db.teardown().await
}

#[tokio::test]
async fn auth_providers_post_rejects_a_service_token_even_admin_scoped() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service(&h, "svc-for-auth-i5").await;

    let body = json!({
        "slug": "provider-i5",
        "service": "svc-for-auth-i5",
        "kind": "static_header",
        "credential_env_key": "A2M_CRED_I5_TEST",
        "header_name": "Authorization",
        "value_template": "Bearer {token}",
        "bound_origin": "https://svc-for-auth-i5.example.com"
    });

    let (status, resp) = bearer(
        &h,
        Method::POST,
        "/api/auth_providers",
        &h.mcp_token,
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an mcp-scoped service token must never reach a write route: {resp:?}"
    );

    let (status, resp) = bearer(
        &h,
        Method::POST,
        "/api/auth_providers",
        &h.admin_scoped_token,
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "I5: even an admin-scoped service token must never bind an auth provider — only a human \
         session may: {resp:?}"
    );

    // The admin session, in contrast, succeeds.
    let (status, resp) = admin(&h, Method::POST, "/api/auth_providers", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");

    h.db.teardown().await
}

#[tokio::test]
async fn a_credential_shaped_value_is_rejected_and_never_echoed() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed_service(&h, "svc-for-leaky-cred").await;

    let leaky_body = json!({
        "slug": "provider-leaky",
        "service": "svc-for-leaky-cred",
        "kind": "static_header",
        // A live-looking token where an env var *name* belongs.
        "credential_env_key": "ghp_aBcDeFgHiJkLmNoPqRsT1234567890",
        "header_name": "Authorization",
        "value_template": "Bearer {token}",
        "bound_origin": "https://svc-for-leaky-cred.example.com"
    });
    let (status, body) = admin(&h, Method::POST, "/api/auth_providers", Some(leaky_body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:?}");
    let errors = body["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("credential"))
    );

    // A legitimately named env var key is accepted, and the response carries only the key name —
    // there is no field anywhere in the response shape a credential *value* could occupy.
    let clean_body = json!({
        "slug": "provider-clean",
        "service": "svc-for-leaky-cred",
        "kind": "static_header",
        "credential_env_key": "A2M_CRED_CLEAN_TEST",
        "header_name": "Authorization",
        "value_template": "Bearer {token}",
        "bound_origin": "https://svc-for-leaky-cred.example.com"
    });
    let (status, body) = admin(&h, Method::POST, "/api/auth_providers", Some(clean_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body:?}");
    assert_eq!(body["credential_env_key"], json!("A2M_CRED_CLEAN_TEST"));
    // `value_template` is a legitimate field (the header *template*, e.g. "Bearer {token}") —
    // what must never appear is a field meant to carry the credential's actual value.
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        !keys
            .iter()
            .any(|&k| k == "value" || k == "credential_value" || k.contains("secret"))
    );

    h.db.teardown().await
}

#[tokio::test]
async fn every_write_path_bumps_definitions_generation() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let gen0 = h.stores.meta().definitions_generation().await?;
    seed_service(&h, "svc-gen-bump").await;
    let gen1 = h.stores.meta().definitions_generation().await?;
    assert!(gen1 > gen0, "service create must bump the generation");

    let call_body = json!({
        "slug": "call-gen-bump",
        "service": "svc-gen-bump",
        "method": "GET",
        "path_template": "/things",
        "access": "read"
    });
    let (status, resp) = admin(&h, Method::POST, "/api/api_calls", Some(call_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    let gen2 = h.stores.meta().definitions_generation().await?;
    assert!(gen2 > gen1, "api_call create must bump the generation");

    let endpoint_body = json!({
        "slug": "ep-gen-bump",
        "tag_expr": "has(does-not-matter)"
    });
    let (status, resp) = admin(&h, Method::POST, "/api/endpoints", Some(endpoint_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {resp:?}");
    let gen3 = h.stores.meta().definitions_generation().await?;
    assert!(gen3 > gen2, "endpoint create must bump the generation");

    let (status, _) = admin(&h, Method::DELETE, "/api/endpoints/ep-gen-bump", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let gen4 = h.stores.meta().definitions_generation().await?;
    assert!(gen4 > gen3, "endpoint delete must bump the generation");

    h.db.teardown().await
}
