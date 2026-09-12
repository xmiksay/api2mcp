//! Integration tests for `POST/GET /api/tokens` and `DELETE /api/tokens/{id}` —
//! `server::api::tokens`'s self-service access-token routes.

mod api_support;
mod common;
mod fixture;

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::store::NewUser;

use api_support::{admin, bearer, setup};

#[tokio::test]
async fn mint_list_and_revoke_roundtrip() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let create_body = json!({ "label": "ci", "expires_in_days": null, "endpoints": [] });
    let (status, minted) = admin(&h, Method::POST, "/api/tokens", Some(create_body)).await;
    assert_eq!(status, StatusCode::CREATED, "body: {minted:?}");
    assert_eq!(minted["label"], json!("ci"));
    assert_eq!(minted["expires_at"], json!(null));
    assert_eq!(minted["endpoints"], json!([]));
    let plaintext = minted["token"]
        .as_str()
        .expect("token is a string")
        .to_owned();
    assert!(!plaintext.is_empty());
    let id = minted["id"].as_str().expect("id is a string").to_owned();

    let (status, listed) = admin(&h, Method::GET, "/api/tokens", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = listed.as_array().expect("array response");
    let row = rows
        .iter()
        .find(|r| r["id"] == json!(id))
        .expect("minted token appears in the list");
    // The plaintext never appears anywhere in the list response — only the store's own
    // `token_prefix` does.
    assert!(row.get("token").is_none());
    assert_eq!(row["token_prefix"], minted["token_prefix"]);
    assert_eq!(row["revoked_at"], json!(null));
    let body_str = listed.to_string();
    assert!(!body_str.contains(&plaintext));

    let (status, _) = admin(&h, Method::DELETE, &format!("/api/tokens/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, listed) = admin(&h, Method::GET, "/api/tokens", None).await;
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(id))
        .expect("revoked token still appears in the list");
    assert!(
        row["revoked_at"].is_string(),
        "expected revoked_at to be set"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn mint_restricted_to_an_endpoint_round_trips_its_grant_list() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    // `setup()` doesn't seed any endpoint, so an unknown slug exercises the same "definer
    // error, not a raw FK violation" path as a real one would — this only checks the wire
    // shape, not resolution against a live endpoint (covered by `tests/mcp_endpoint_grants.rs`
    // and `tests/store_service_token_endpoints.rs`).
    let create_body =
        json!({ "label": "scoped", "expires_in_days": 30, "endpoints": ["no-such-endpoint"] });
    let (status, body) = admin(&h, Method::POST, "/api/tokens", Some(create_body)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "minting against an unknown endpoint slug should be a 400, not a raw db error: {body:?}"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn a_service_token_cannot_reach_any_tokens_route() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let (status, _) = bearer(&h, Method::GET, "/api/tokens", &h.mcp_token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = bearer(
        &h,
        Method::POST,
        "/api/tokens",
        &h.mcp_token,
        Some(json!({ "label": "should-not-mint", "expires_in_days": null, "endpoints": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = bearer(
        &h,
        Method::DELETE,
        "/api/tokens/00000000-0000-0000-0000-000000000000",
        &h.mcp_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    h.db.teardown().await
}

#[tokio::test]
async fn a_user_cannot_see_or_revoke_another_users_token() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let other = h
        .stores
        .user()
        .create(NewUser {
            email: "other-owner@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;
    let other_token = h
        .stores
        .service_token()
        .mint(
            other.id,
            "someone elses token".to_owned(),
            None,
            Default::default(),
        )
        .await?;

    let (_, listed) = admin(&h, Method::GET, "/api/tokens", None).await;
    let rows = listed.as_array().expect("array response");
    assert!(
        !rows
            .iter()
            .any(|r| r["id"] == json!(other_token.record.id.to_string())),
        "another user's token must not appear in this caller's list"
    );

    let (status, _) = admin(
        &h,
        Method::DELETE,
        &format!("/api/tokens/{}", other_token.record.id),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "revoking another user's token id must 404, not 403"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn revoking_an_unknown_token_id_404s() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let (status, _) = admin(
        &h,
        Method::DELETE,
        "/api/tokens/00000000-0000-0000-0000-000000000000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    h.db.teardown().await
}
