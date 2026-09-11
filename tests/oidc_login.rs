//! End-to-end tests for Decision 2's OIDC login (`GET /login/oidc/start`, `GET
//! /login/oidc/callback`), driven through the real router (`tower::ServiceExt::oneshot`)
//! against [`idp::MockIdp`] — a genuine listener, since `server::oidc` reaches it over
//! `reqwest`. Skipped when `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).

mod common;
mod fixture;

#[path = "oidc_support/idp.rs"]
mod idp;
#[path = "oauth_support/wire.rs"]
mod wire;

use std::sync::Arc;

use anyhow::Result;
use axum::http::{HeaderMap, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sea_orm::EntityTrait;

use api2mcp::config::Config;
use api2mcp::entity::{sessions, users};
use api2mcp::resolve::PlanCache;
use api2mcp::server::auth::SESSION_COOKIE_NAME;
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, UserStore};

use common::ScratchDb;
use fixture::harness::loopback_pool;
use idp::MockIdp;

const FLOW_COOKIE: &str = "a2m_oidc_flow";
const CLIENT_SECRET: &str = "s3cr3t-value-that-must-never-leak";

async fn build_state(idp: &MockIdp, db_url_conn: &sea_orm::DatabaseConnection) -> Result<AppState> {
    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_BASE_URL" => Some("http://test.local:8080".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("default".to_owned()),
        "A2M_ALLOW_LOOPBACK_UPSTREAM" => Some("1".to_owned()),
        "A2M_OIDC_ISSUER" => Some(idp.base_url.clone()),
        "A2M_OIDC_CLIENT_ID" => Some("test-client".to_owned()),
        "A2M_OIDC_CLIENT_SECRET" => Some(CLIENT_SECRET.to_owned()),
        _ => None,
    })?;
    Ok(AppState::new(
        db_url_conn.clone(),
        Arc::new(cfg),
        Arc::new(loopback_pool()),
        Arc::new(PlanCache::new()),
    ))
}

fn set_cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::SET_COOKIE).iter().find_map(|v| {
        let s = v.to_str().ok()?;
        let (k, rest) = s.split_once('=')?;
        (k == name && !rest.starts_with(';'))
            .then(|| rest.split(';').next().unwrap_or("").to_owned())
    })
}

/// Same wire format `server::login_oidc`'s private `FlowState` uses (`state`/`verifier`/`next`,
/// base64url JSON) — reconstructed here rather than imported, since it's `src/`-private; this
/// is exactly what a black-box HTTP test is allowed to know (the cookie's wire shape), not an
/// internal type.
fn encode_flow_cookie(state: &str, verifier: &str, next: &str) -> String {
    let json = serde_json::json!({"state": state, "verifier": verifier, "next": next});
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).expect("serializes"))
}

struct StartResult {
    state: String,
    code_challenge: String,
    flow_cookie: String,
}

async fn do_start(router: &axum::Router, next: &str) -> StartResult {
    let (status, headers, _) = wire::get(
        router,
        &format!("/login/oidc/start?next={}", wire::urlenc(next)),
        None,
    )
    .await;
    assert!(status.is_redirection(), "expected a redirect, got {status}");
    let location = wire::location(&headers);
    let url = url::Url::parse(&location).expect("authorize url");
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let flow_cookie =
        set_cookie_value(&headers, FLOW_COOKIE).expect("start sets the oidc flow cookie");
    StartResult {
        state: pairs["state"].clone(),
        code_challenge: pairs["code_challenge"].clone(),
        flow_cookie,
    }
}

#[tokio::test]
async fn full_login_flow_creates_a_session_and_never_leaks_the_client_secret() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;

    let idp = MockIdp::start().await;
    let state = build_state(&idp, &db.conn).await?;
    let router = build_router(state.clone());

    let start = do_start(&router, "/dest").await;
    idp.expect_code_challenge(&start.code_challenge);
    idp.set_identity("sub-happy-path", "person@example.com");

    let cookie = format!("{FLOW_COOKIE}={}", start.flow_cookie);
    let callback_uri = format!(
        "/login/oidc/callback?code=fake-code&state={}",
        wire::urlenc(&start.state)
    );
    let (status, headers, body) = wire::get(&router, &callback_uri, Some(&cookie)).await;
    assert!(status.is_redirection(), "expected a redirect, got {status}");
    assert_eq!(wire::location(&headers), "/dest");
    let session_cookie =
        set_cookie_value(&headers, SESSION_COOKIE_NAME).expect("callback issues a session cookie");

    // The identity was created, matched by (issuer, subject).
    let row = users::Entity::find()
        .one(&state.db)
        .await?
        .expect("the oidc sign-in created a user");
    assert_eq!(row.email, "person@example.com");
    assert_eq!(row.oidc_subject.as_deref(), Some("sub-happy-path"));
    assert_eq!(row.oidc_issuer.as_deref(), Some(idp.base_url.as_str()));
    assert!(row.password_hash.is_none());

    // The session cookie actually authenticates on the admin API.
    let me_cookie = format!("{SESSION_COOKIE_NAME}={session_cookie}");
    let (status, _, me_body) = wire::get(&router, "/api/me", Some(&me_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let me: serde_json::Value = serde_json::from_slice(&me_body)?;
    assert_eq!(me["kind"], serde_json::json!("session"));

    // The client secret was genuinely sent to the provider (client_secret_basic, RFC 6749
    // §2.3.1) — proving absence elsewhere isn't just because it was never used at all.
    let expected_basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("test-client:{CLIENT_SECRET}"))
    );
    assert!(
        idp.seen_token_auth_headers().contains(&expected_basic),
        "the token endpoint never saw the expected client_secret_basic header"
    );

    // But it never leaks anywhere else: not in this whole flow's response bodies, not in the
    // `Config`'s own `Debug` (what an accidental `tracing::debug!(?cfg)` would print), and not
    // in any row of the tables this flow just wrote to.
    assert!(!String::from_utf8_lossy(&body).contains(CLIENT_SECRET));
    assert!(!String::from_utf8_lossy(&me_body).contains(CLIENT_SECRET));
    assert!(!format!("{:?}", state.cfg.oidc).contains(CLIENT_SECRET));
    let user_rows = users::Entity::find().all(&state.db).await?;
    assert!(!format!("{user_rows:?}").contains(CLIENT_SECRET));
    let session_rows = sessions::Entity::find().all(&state.db).await?;
    assert!(!format!("{session_rows:?}").contains(CLIENT_SECRET));

    db.teardown().await
}

#[tokio::test]
async fn callback_rejects_a_state_that_does_not_match_the_flow_cookie() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;

    let idp = MockIdp::start().await;
    let state = build_state(&idp, &db.conn).await?;
    let router = build_router(state.clone());

    let start = do_start(&router, "/dest").await;
    idp.expect_code_challenge(&start.code_challenge);
    idp.set_identity("sub-state-mismatch", "state-mismatch@example.com");

    let cookie = format!("{FLOW_COOKIE}={}", start.flow_cookie);
    // The query string's `state` does not match the one the flow cookie carries.
    let (status, headers, _) = wire::get(
        &router,
        "/login/oidc/callback?code=fake-code&state=not-the-real-state",
        Some(&cookie),
    )
    .await;
    assert!(status.is_redirection());
    let location = wire::location(&headers);
    assert!(location.starts_with("/login"), "got {location}");
    assert!(
        set_cookie_value(&headers, SESSION_COOKIE_NAME).is_none(),
        "a state mismatch must never issue a session cookie"
    );
    assert!(
        users::Entity::find().one(&state.db).await?.is_none(),
        "a state mismatch must never create a user"
    );

    db.teardown().await
}

#[tokio::test]
async fn callback_rejects_a_code_verifier_that_does_not_match_the_challenge() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;

    let idp = MockIdp::start().await;
    let state = build_state(&idp, &db.conn).await?;
    let router = build_router(state.clone());

    let start = do_start(&router, "/dest").await;
    // The idp only ever learns the *real* code_challenge — exactly what a real provider would
    // have bound to the authorization code at its own `/authorize` step.
    idp.expect_code_challenge(&start.code_challenge);
    idp.set_identity("sub-bad-verifier", "bad-verifier@example.com");

    // A tampered flow cookie: the real `state` (so the state check passes) but a `verifier`
    // that does not hash to the `code_challenge` already sent to the provider — e.g. an
    // attacker who obtained the authorization code but not the verifier that produced its
    // challenge.
    let tampered_cookie = format!(
        "{FLOW_COOKIE}={}",
        encode_flow_cookie(
            &start.state,
            "a-verifier-that-does-not-match-the-challenge",
            "/dest"
        )
    );
    let callback_uri = format!(
        "/login/oidc/callback?code=fake-code&state={}",
        wire::urlenc(&start.state)
    );
    let (status, headers, _) = wire::get(&router, &callback_uri, Some(&tampered_cookie)).await;
    assert!(status.is_redirection());
    assert!(wire::location(&headers).starts_with("/login"));
    assert!(
        set_cookie_value(&headers, SESSION_COOKIE_NAME).is_none(),
        "a PKCE verifier mismatch must never issue a session cookie"
    );
    assert!(
        users::Entity::find().one(&state.db).await?.is_none(),
        "a PKCE verifier mismatch must never create a user"
    );

    db.teardown().await
}

/// The end-to-end shape of the task's headline fix: `api2mcp user add --oidc-only` pre-provisions
/// a row with no way to log in; the person it was made for signs in over real OIDC and the
/// callback claims that row instead of creating a second account next to it.
#[tokio::test]
async fn full_login_flow_claims_a_pending_oidc_only_account_instead_of_duplicating_it() -> Result<()>
{
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;

    let idp = MockIdp::start().await;
    let state = build_state(&idp, &db.conn).await?;
    let router = build_router(state.clone());

    let pending = UserStore::new(state.db.clone())
        .create_pending_oidc("invitee@example.com")
        .await?;

    let start = do_start(&router, "/dest").await;
    idp.expect_code_challenge(&start.code_challenge);
    idp.set_identity("sub-invitee", "invitee@example.com");

    let cookie = format!("{FLOW_COOKIE}={}", start.flow_cookie);
    let callback_uri = format!(
        "/login/oidc/callback?code=fake-code&state={}",
        wire::urlenc(&start.state)
    );
    let (status, headers, _) = wire::get(&router, &callback_uri, Some(&cookie)).await;
    assert!(status.is_redirection(), "expected a redirect, got {status}");
    assert!(
        set_cookie_value(&headers, SESSION_COOKIE_NAME).is_some(),
        "the claim must still issue a session cookie"
    );

    // Exactly one row, claimed rather than duplicated: same id as the pending invite, now linked.
    let all_users = users::Entity::find().all(&state.db).await?;
    assert_eq!(
        all_users.len(),
        1,
        "the pending row must be claimed, not duplicated"
    );
    assert_eq!(all_users[0].id, pending.id);
    assert_eq!(all_users[0].oidc_subject.as_deref(), Some("sub-invitee"));
    assert_eq!(
        all_users[0].oidc_issuer.as_deref(),
        Some(idp.base_url.as_str())
    );

    // A second sign-in resolves the now-linked account by identity, still one row.
    let start2 = do_start(&router, "/dest").await;
    idp.expect_code_challenge(&start2.code_challenge);
    let cookie2 = format!("{FLOW_COOKIE}={}", start2.flow_cookie);
    let callback_uri2 = format!(
        "/login/oidc/callback?code=fake-code&state={}",
        wire::urlenc(&start2.state)
    );
    let (status2, headers2, _) = wire::get(&router, &callback_uri2, Some(&cookie2)).await;
    assert!(status2.is_redirection());
    assert!(set_cookie_value(&headers2, SESSION_COOKIE_NAME).is_some());
    assert_eq!(users::Entity::find().all(&state.db).await?.len(), 1);

    db.teardown().await
}

/// The other half of the fix: an unverified email must never let a sign-in claim someone else's
/// account. The login simply fails (a clean conflict, logged server-side — see
/// `server::login_oidc`'s own doc for why the browser never sees why); the existing password
/// account is left completely untouched.
#[tokio::test]
async fn full_login_flow_refuses_to_claim_an_account_when_the_email_is_not_verified() -> Result<()>
{
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;

    let idp = MockIdp::start().await;
    let state = build_state(&idp, &db.conn).await?;
    let router = build_router(state.clone());

    let users = UserStore::new(state.db.clone());
    let password_user = users
        .create(NewUser {
            email: "unverified-target@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    let start = do_start(&router, "/dest").await;
    idp.expect_code_challenge(&start.code_challenge);
    idp.set_identity_verified(
        "sub-not-verified",
        "unverified-target@example.com",
        Some(false),
    );

    let cookie = format!("{FLOW_COOKIE}={}", start.flow_cookie);
    let callback_uri = format!(
        "/login/oidc/callback?code=fake-code&state={}",
        wire::urlenc(&start.state)
    );
    let (status, headers, _) = wire::get(&router, &callback_uri, Some(&cookie)).await;
    assert!(status.is_redirection());
    assert!(wire::location(&headers).starts_with("/login"));
    assert!(
        set_cookie_value(&headers, SESSION_COOKIE_NAME).is_none(),
        "an unverified email must never issue a session cookie"
    );

    // The account is untouched: still one row, still unlinked, password still works.
    let all_users = users::Entity::find().all(&state.db).await?;
    assert_eq!(all_users.len(), 1);
    assert_eq!(all_users[0].id, password_user.id);
    assert!(all_users[0].oidc_issuer.is_none());
    assert!(
        UserStore::new(state.db.clone())
            .verify_password(
                "unverified-target@example.com",
                "correct horse battery staple"
            )
            .await?
            .is_some()
    );

    db.teardown().await
}
