//! Integration tests for chunk C12's `refresh_token` grant: rotation, family reuse
//! detection, and the absolute family TTL — split out of `tests/oauth.rs` to respect the
//! workspace's 400-line cap, same as the `server::oauth::refresh` module these exercise.
//! Skipped when `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).

mod common;

#[path = "oauth_support/wire.rs"]
mod wire;

use anyhow::Result;
use axum::Router;
use axum::http::StatusCode;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::Value;

use api2mcp::config::Config;
use api2mcp::resolve::PlanCache;
use api2mcp::server::auth::SESSION_COOKIE_NAME;
use api2mcp::server::{self, AppState};
use api2mcp::store::{NewOauthClient, NewUser, Stores};

use common::ScratchDb;
use wire::{get, location, pkce_pair, post_form, urldecode, urlenc};

const REDIRECT_URI: &str = "http://client.example/callback";

struct Harness {
    db: ScratchDb,
    router: Router,
}

/// Registers a client + user, drives the authorize/consent/code/token dance once through the
/// real HTTP router, and returns the resulting `(access_token, refresh_token)` — every test in
/// this file only cares about what happens *after* that pair exists.
async fn setup_with_initial_pair() -> Result<Option<(Harness, String, String)>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "refresh@example.com".into(),
            password: "correct horse battery staple".into(),
            is_admin: false,
        })
        .await?;
    let session = api2mcp::server::auth::create_session(
        &db.conn,
        user.id,
        std::time::Duration::from_secs(3600),
    )
    .await?;
    let cookie = format!("{SESSION_COOKIE_NAME}={session}");

    let (client, _) = stores
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

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_BASE_URL" => Some("http://test.local".to_owned()),
        _ => None,
    })?;
    let state = AppState::new(
        db.conn.clone(),
        std::sync::Arc::new(cfg),
        std::sync::Arc::new(dummy_upstream_pool()),
        std::sync::Arc::new(PlanCache::new()),
    );
    let router = Router::new()
        .merge(server::oauth::router())
        .with_state(state);

    let (verifier, challenge) = pkce_pair();
    let authorize_uri = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256",
        client.id,
        urlenc(REDIRECT_URI),
        urlenc(&challenge),
    );
    let (_status, headers, _body) = get(&router, &authorize_uri, Some(&cookie)).await;
    let consent_uri = location(&headers);
    let request_id = consent_uri
        .strip_prefix("/oauth/consent?request_id=")
        .expect("request_id in the redirect");
    let form = format!("request_id={request_id}&decision=approve");
    let (_status, headers, _body) =
        post_form(&router, "/oauth/consent", &form, Some(&cookie)).await;
    let redirect = location(&headers);
    let code = urldecode(
        redirect
            .split("code=")
            .nth(1)
            .expect("code param")
            .split('&')
            .next()
            .expect("code value"),
    );

    let token_body = format!("grant_type=authorization_code&code={code}&code_verifier={verifier}");
    let (status, _headers, body) = post_form(&router, "/oauth/token", &token_body, None).await;
    assert_eq!(status, StatusCode::OK, "initial code exchange must succeed");
    let doc: Value = serde_json::from_slice(&body)?;
    let access = doc["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned();
    let refresh = doc["refresh_token"]
        .as_str()
        .expect("refresh_token")
        .to_owned();

    Ok(Some((Harness { db, router }, access, refresh)))
}

/// No outbound HTTP happens in any test in this file (only the token endpoint is exercised),
/// so an empty pool with no DNS backend is a safe placeholder — `AppState` still needs one.
fn dummy_upstream_pool() -> api2mcp::http::UpstreamPool {
    api2mcp::http::UpstreamPool::new(
        std::sync::Arc::new(api2mcp::http::StaticDns::new()),
        api2mcp::http::SsrfPolicy {
            allow_loopback: true,
        },
    )
}

async fn refresh(router: &Router, refresh_token: &str) -> (StatusCode, Value) {
    let body = format!("grant_type=refresh_token&refresh_token={refresh_token}");
    let (status, _headers, body) = post_form(router, "/oauth/token", &body, None).await;
    let doc = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, doc)
}

#[tokio::test]
async fn refresh_rotates_and_the_old_token_stops_working() -> Result<()> {
    let Some((h, _access, refresh_token)) = setup_with_initial_pair().await? else {
        return Ok(());
    };

    let (status, doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::OK);
    let new_refresh = doc["refresh_token"]
        .as_str()
        .expect("a rotated refresh_token")
        .to_owned();
    assert_ne!(new_refresh, refresh_token, "rotation must mint a new value");
    assert!(doc["access_token"].as_str().is_some());

    // The old refresh token is gone the moment it rotated — using it again is reuse, not "one
    // of two valid tokens".
    let (status, _doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    h.db.teardown().await
}

#[tokio::test]
async fn reusing_a_rotated_refresh_token_revokes_the_whole_family() -> Result<()> {
    let Some((h, _access, refresh_token)) = setup_with_initial_pair().await? else {
        return Ok(());
    };

    let (status, doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::OK);
    let rotated = doc["refresh_token"]
        .as_str()
        .expect("rotated token")
        .to_owned();

    // Replaying the now-revoked original: refused, and must revoke the *entire* family —
    // the one behaviour that turns a stolen token from a persistent compromise into a
    // detected one.
    let (status, _doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Proof the whole family died, not just the reused row: the token that *did* rotate
    // successfully a moment ago must now also be dead.
    let (status, _doc) = refresh(&h.router, &rotated).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "reuse must revoke every token in the family, including ones that rotated cleanly"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn a_family_older_than_the_absolute_ttl_is_refused_and_fully_revoked() -> Result<()> {
    let Some((h, _access, refresh_token)) = setup_with_initial_pair().await? else {
        return Ok(());
    };

    // Backdate every row belonging to this token's family so its origin looks 31 days old —
    // there is no store API to do this (rows are always inserted with `now()`), and doing it
    // via raw SQL against the scratch DB is the established pattern for this kind of
    // otherwise-unreachable state (see `tests/store.rs`'s malformed-JSONB test).
    let backdated = Utc::now() - chrono::Duration::days(31);
    h.db.conn
        .execute(Statement::from_sql_and_values(
            h.db.conn.get_database_backend(),
            "UPDATE oauth_tokens SET created_at = $1",
            [backdated.into()],
        ))
        .await?;

    let (status, doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(doc["error"], serde_json::json!("invalid_grant"));

    // The family must be dead outright, not merely "this attempt failed" — a second, entirely
    // fresh presentation of the same (still only known) refresh token must also fail.
    let (status, _doc) = refresh(&h.router, &refresh_token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    h.db.teardown().await
}
