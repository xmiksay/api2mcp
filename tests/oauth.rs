//! Integration tests for chunk C12's OAuth 2.1 authorization server: discovery metadata,
//! dynamic client registration, and `/oauth/authorize` request validation — driven through
//! [`tower::ServiceExt::oneshot`] against a router built from `server::oauth::router()`.
//! Skipped when `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).
//!
//! The full authorize→consent→token round trip, PKCE verifier enforcement, and code-replay
//! protection live in `tests/oauth_flow.rs`; refresh-grant rotation/reuse/absolute-TTL live in
//! `tests/oauth_refresh.rs` — both split out, along with the shared `oauth_wire`/
//! `oauth_harness` helper modules below, to respect the workspace's 400-line cap.

mod common;
mod fixture;

#[path = "oauth_support/harness.rs"]
mod harness;
#[path = "oauth_support/wire.rs"]
mod wire;

use anyhow::Result;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use api2mcp::store::NewOauthClient;

use harness::{REDIRECT_URI, login_cookie, setup};
use wire::{get, location, pkce_pair, qs, send};

#[tokio::test]
async fn discovery_documents_agree_with_what_a_401_advertises() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let (status, _headers, body) =
        get(&h.router, "/.well-known/oauth-protected-resource", None).await;
    assert_eq!(status, StatusCode::OK);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["resource"], json!(format!("{}/mcp", h.cfg.base_url)));
    assert_eq!(doc["authorization_servers"], json!([h.cfg.base_url]));

    let (status, _headers, body) =
        get(&h.router, "/.well-known/oauth-authorization-server", None).await;
    assert_eq!(status, StatusCode::OK);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["issuer"], json!(h.cfg.base_url));
    assert_eq!(
        doc["authorization_endpoint"],
        json!(format!("{}/oauth/authorize", h.cfg.base_url))
    );
    assert_eq!(
        doc["token_endpoint"],
        json!(format!("{}/oauth/token", h.cfg.base_url))
    );
    assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));

    // What a real 401 on /mcp advertises must resolve to the exact document above.
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .expect("valid request");
    let (status, headers, _body) = send(&h.router, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let www_auth = headers
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate on a 401")
        .to_str()?;
    assert_eq!(
        www_auth,
        format!(
            r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource""#,
            h.cfg.base_url
        )
    );

    h.db.teardown().await
}

#[tokio::test]
async fn dynamic_client_registration_round_trips_and_rejects_no_redirect_uris() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let body = json!({ "redirect_uris": [REDIRECT_URI], "client_name": "test client" });
    let request = Request::builder()
        .method("POST")
        .uri("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body)?))
        .expect("valid request");
    let (status, _headers, body) = send(&h.router, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["token_endpoint_auth_method"], json!("none"));
    assert!(doc["client_id"].as_str().is_some());
    assert!(
        doc.get("client_secret").is_none(),
        "a public client must never get a secret"
    );

    let empty = json!({ "redirect_uris": [] });
    let request = Request::builder()
        .method("POST")
        .uri("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&empty)?))
        .expect("valid request");
    let (status, _headers, body) = send(&h.router, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["error"], json!("invalid_redirect_uri"));

    h.db.teardown().await
}

#[tokio::test]
async fn unauthenticated_authorize_bounces_to_login_with_a_safe_next() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let (client, _) = h
        .stores
        .oauth()
        .register_client(
            NewOauthClient {
                client_name: "cc".into(),
                redirect_uris: vec![REDIRECT_URI.into()],
                grant_types: vec!["authorization_code".into(), "refresh_token".into()],
                token_endpoint_auth_method: "none".into(),
                scope: None,
            },
            false,
        )
        .await?;
    let (_verifier, challenge) = pkce_pair();

    let uri = format!(
        "/oauth/authorize?{}",
        qs(&[
            ("response_type", "code"),
            ("client_id", &client.id.to_string()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
    );
    let (status, headers, _body) = get(&h.router, &uri, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let loc = location(&headers);
    assert!(loc.starts_with("/login?next="), "got {loc:?}");
    assert!(
        loc.contains("%2Foauth%2Fauthorize"),
        "next must carry the original authorize path, got {loc:?}"
    );
    assert!(
        !loc.contains("//") || loc.starts_with("/login?next=%2F"),
        "next must never decode to a protocol-relative or absolute URL"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn pkce_plain_is_refused() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let (client, _) = h
        .stores
        .oauth()
        .register_client(
            NewOauthClient {
                client_name: "cc".into(),
                redirect_uris: vec![REDIRECT_URI.into()],
                grant_types: vec!["authorization_code".into()],
                token_endpoint_auth_method: "none".into(),
                scope: None,
            },
            false,
        )
        .await?;
    let cookie = login_cookie(&h.stores, &h.db.conn, "plain@example.com").await?;

    let uri = format!(
        "/oauth/authorize?{}",
        qs(&[
            ("response_type", "code"),
            ("client_id", &client.id.to_string()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", "somechallenge"),
            ("code_challenge_method", "plain"),
        ])
    );
    let (status, _headers, body) = get(&h.router, &uri, Some(&cookie)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["error"], json!("invalid_request"));

    h.db.teardown().await
}
