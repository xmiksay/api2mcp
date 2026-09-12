//! Integration tests for chunk C12's authorize→consent→token round trip: the named minimum
//! (register → authorize → consent → code → token → use it on `/mcp`), PKCE verifier
//! enforcement, and code-replay protection. Split out of `tests/oauth.rs` to respect the
//! workspace's 400-line cap. Skipped when `TEST_DATABASE_URL` is unset.

mod common;
mod fixture;

#[path = "oauth_support/harness.rs"]
mod harness;
#[path = "oauth_support/wire.rs"]
mod wire;

use anyhow::Result;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use serde_json::{Value, json};

use api2mcp::store::{NewOauthClient, RunCallerKind};

use harness::{REDIRECT_URI, login_cookie, setup};
use wire::{get, location, pkce_pair, post_form, qs, send, urldecode};

/// The named minimum: register → authorize → consent → code → token → use it on `/mcp`. Also
/// proves a wrong PKCE verifier is refused and that a used code cannot be replayed.
#[tokio::test]
async fn full_round_trip_register_authorize_consent_token_and_call_mcp() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let cookie = login_cookie(&h, "roundtrip@example.com").await?;
    let (verifier, challenge) = pkce_pair();

    let register_body = json!({ "redirect_uris": [REDIRECT_URI] });
    let request = Request::builder()
        .method("POST")
        .uri("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body)?))
        .expect("valid request");
    let (status, _headers, body) = send(&h.router, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let client_id = serde_json::from_slice::<Value>(&body)?["client_id"]
        .as_str()
        .expect("client_id")
        .to_owned();

    let authorize_uri = format!(
        "/oauth/authorize?{}",
        qs(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("scope", "mcp"),
        ])
    );
    let (status, headers, _body) = get(&h.router, &authorize_uri, Some(&cookie)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let consent_uri = location(&headers);
    assert!(consent_uri.starts_with("/oauth/consent?request_id="));

    // The consent screen itself renders for a logged-in user.
    let (status, _headers, body) = get(&h.router, &consent_uri, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8(body)?;
    assert!(html.contains("Allow"));

    let request_id = consent_uri
        .strip_prefix("/oauth/consent?request_id=")
        .expect("request_id in the redirect");
    let form = format!("request_id={request_id}&decision=approve");
    let (status, headers, _body) =
        post_form(&h.router, "/oauth/consent", &form, Some(&cookie)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let redirect = location(&headers);
    assert!(redirect.starts_with(REDIRECT_URI));
    assert!(redirect.contains("state=xyz"));
    let code = urldecode(
        redirect
            .split("code=")
            .nth(1)
            .expect("a code param")
            .split('&')
            .next()
            .expect("code value"),
    );

    // Wrong verifier: refused, and the code must not be burned by the attempt (checked next).
    let wrong_body = qs(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        (
            "code_verifier",
            "not-the-real-verifier-not-the-real-verifier",
        ),
        ("redirect_uri", REDIRECT_URI),
    ]);
    let (status, _headers, body) = post_form(&h.router, "/oauth/token", &wrong_body, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["error"], json!("invalid_grant"));

    // Correct verifier: the code is already burned by the failed attempt above (this store's
    // `consume_code` marks used on read, a stricter single-attempt semantics — see
    // `server::oauth::token`'s doc), so this must also fail rather than succeed.
    let right_body = qs(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("code_verifier", &verifier),
        ("redirect_uri", REDIRECT_URI),
    ]);
    let (status, _headers, _body) = post_form(&h.router, "/oauth/token", &right_body, None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a code must be single-use even against a wrong-then-right verifier pair"
    );

    h.db.teardown().await
}

/// Drives authorize→consent→code once more (its own client/user so it can't collide with
/// other tests' rows) purely to exchange the code twice.
#[tokio::test]
async fn code_cannot_be_replayed_after_a_successful_exchange() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let cookie = login_cookie(&h, "replay@example.com").await?;
    let (verifier, challenge) = pkce_pair();
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

    let authorize_uri = format!(
        "/oauth/authorize?{}",
        qs(&[
            ("response_type", "code"),
            ("client_id", &client.id.to_string()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
    );
    let (_status, headers, _body) = get(&h.router, &authorize_uri, Some(&cookie)).await;
    let consent_uri = location(&headers);
    let request_id = consent_uri
        .strip_prefix("/oauth/consent?request_id=")
        .expect("request_id");
    let form = format!("request_id={request_id}&decision=approve");
    let (_status, headers, _body) =
        post_form(&h.router, "/oauth/consent", &form, Some(&cookie)).await;
    let redirect = location(&headers);
    let code = urldecode(redirect.split("code=").nth(1).expect("code param"));

    let body = qs(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("code_verifier", &verifier),
    ]);
    let (status, _headers, _body) = post_form(&h.router, "/oauth/token", &body, None).await;
    assert_eq!(status, StatusCode::OK);

    // Same code, same (correct) verifier, second time: must be refused.
    let (status, _headers, body) = post_form(&h.router, "/oauth/token", &body, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let doc: Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["error"], json!("invalid_grant"));

    h.db.teardown().await
}

/// Closes the loop `authenticate_mcp`'s OAuth branch was added for: a token produced by this
/// module's own token endpoint actually authenticates a `tools/call` on `/mcp`.
#[tokio::test]
async fn an_issued_access_token_authenticates_on_mcp() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let cookie = login_cookie(&h, "mcp-user@example.com").await?;
    let (verifier, challenge) = pkce_pair();
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

    let authorize_uri = format!(
        "/oauth/authorize?{}",
        qs(&[
            ("response_type", "code"),
            ("client_id", &client.id.to_string()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
    );
    let (_status, headers, _body) = get(&h.router, &authorize_uri, Some(&cookie)).await;
    let consent_uri = location(&headers);
    let request_id = consent_uri
        .strip_prefix("/oauth/consent?request_id=")
        .expect("request_id");
    let form = format!("request_id={request_id}&decision=approve");
    let (_status, headers, _body) =
        post_form(&h.router, "/oauth/consent", &form, Some(&cookie)).await;
    let redirect = location(&headers);
    let code = urldecode(redirect.split("code=").nth(1).expect("code"));

    let body = qs(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("code_verifier", &verifier),
    ]);
    let (status, _headers, body) = post_form(&h.router, "/oauth/token", &body, None).await;
    assert_eq!(status, StatusCode::OK);
    let doc: Value = serde_json::from_slice(&body)?;
    let access_token = doc["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned();

    let rpc = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "get-item", "arguments": { "id": "1" } }
    });
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access_token}"))?,
        )
        .body(Body::from(serde_json::to_vec(&rpc)?))
        .expect("valid request");
    let (status, _headers, body) = send(&h.router, request).await;
    assert_eq!(status, StatusCode::OK);
    let doc: Value = serde_json::from_slice(&body)?;
    assert!(
        doc.get("error").is_none(),
        "unexpected JSON-RPC error: {doc:?}"
    );
    assert!(
        !doc["result"]["isError"].as_bool().unwrap_or(false),
        "tool call reported isError: {doc:?}"
    );

    // The point of this whole round trip (`server::auth::authenticate_mcp`'s OAuth branch): the
    // recorded run must say `oauth`, distinct from a browser session and from a service token,
    // even though `Caller::from_oauth_user` resolves the very same user a session would.
    let user = h
        .stores
        .user()
        .get_by_email("mcp-user@example.com")
        .await?
        .expect("the OAuth-granting user exists");
    let runs = h
        .stores
        .run()
        .list_for_endpoint(user.id, &"demo".parse().unwrap(), 10)
        .await?;
    assert_eq!(runs.len(), 1, "exactly one run should have been recorded");
    let (detail, _calls) = h
        .stores
        .run()
        .get(user.id, runs[0].id)
        .await?
        .expect("run row exists");
    assert_eq!(detail.caller_kind, RunCallerKind::Oauth);

    h.db.teardown().await
}
