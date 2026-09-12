//! `server::auth`/`server::login` integration tests against a real scratch Postgres
//! (skipped when `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`).

mod common;

use anyhow::Result;
use axum::http::{HeaderMap, HeaderValue, header};
use chrono::{Duration as ChronoDuration, Utc};
use common::ScratchDb;
use sea_orm::EntityTrait;

use api2mcp::config::Config;
use api2mcp::entity::{service_tokens, users};
use api2mcp::server::{auth, login};
use api2mcp::store::{NewUser, Stores};

fn test_config() -> Config {
    Config {
        database_url: String::new(),
        host: "127.0.0.1".into(),
        port: 8080,
        base_url: "http://test.local:8080".into(),
        default_endpoint: "default".into(),
        seed_email: None,
        seed_password: None,
        run_retention_days: 30,
        allow_loopback_upstream: false,
        session_ttl: std::time::Duration::from_secs(3600),
        max_request_bytes: 1024 * 1024,
        oidc: None,
    }
}

fn bearer_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).expect("token is always valid ASCII"),
    );
    h
}

#[tokio::test]
async fn minted_token_resolves_revoked_and_expired_do_not() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let cfg = test_config();

    let user = stores
        .user()
        .create(NewUser {
            email: "admin@example.com".into(),
            password: "hunter2hunter2".into(),
        })
        .await?;

    let minted = stores
        .service_token()
        .mint(user.id, "ci".into(), None, Default::default())
        .await?;
    let headers = bearer_headers(&minted.plaintext);

    let (caller, grants) = auth::authenticate_mcp(&db.conn, &cfg, &headers)
        .await
        .map_err(|_| anyhow::anyhow!("expected the minted token to authenticate"))?;
    assert_eq!(caller.id, user.id);
    assert_eq!(
        grants,
        auth::EndpointGrants::All,
        "an unrestricted mint grants every endpoint"
    );

    // Revoked: the same token no longer resolves.
    stores.service_token().revoke(minted.record.id).await?;
    assert!(
        auth::authenticate_mcp(&db.conn, &cfg, &headers)
            .await
            .is_err()
    );

    // Expired: a token minted with a past expiry never resolves, even freshly minted.
    let expired = stores
        .service_token()
        .mint(
            user.id,
            "expiring".into(),
            Some(Utc::now() - ChronoDuration::seconds(1)),
            Default::default(),
        )
        .await?;
    let expired_headers = bearer_headers(&expired.plaintext);
    assert!(
        auth::authenticate_mcp(&db.conn, &cfg, &expired_headers)
            .await
            .is_err()
    );

    db.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn service_tokens_row_never_contains_the_plaintext() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "dump@example.com".into(),
            password: "whatever12345".into(),
        })
        .await?;
    let minted = stores
        .service_token()
        .mint(user.id, "dump-test".into(), None, Default::default())
        .await?;

    let row = service_tokens::Entity::find_by_id(minted.record.id)
        .one(&db.conn)
        .await?
        .expect("row was just inserted");

    // The `Debug` dump of the whole row is the broadest possible check: every field,
    // formatted, in one string.
    let dump = format!("{row:?}");
    assert!(
        !dump.contains(&minted.plaintext),
        "plaintext leaked into a service_tokens row: {dump}"
    );

    // And explicitly, in case a future column addition changes what `Debug` prints.
    assert_ne!(row.token_hash, minted.plaintext);
    assert!(!row.token_hash.contains(&minted.plaintext));
    assert!(!row.token_prefix.contains(&minted.plaintext));
    assert!(!row.label.contains(&minted.plaintext));

    db.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn a_missing_or_bad_bearer_token_gets_a_401_challenge() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let cfg = test_config();
    let expected = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        cfg.base_url
    );

    let no_header = auth::authenticate_mcp(&db.conn, &cfg, &HeaderMap::new())
        .await
        .unwrap_err();
    assert_eq!(no_header.www_authenticate, expected);

    let bad_token = auth::authenticate_mcp(&db.conn, &cfg, &bearer_headers("not-a-real-token"))
        .await
        .unwrap_err();
    assert_eq!(bad_token.www_authenticate, expected);

    db.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn password_verification_succeeds_and_fails_and_the_stored_hash_is_not_the_password()
-> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let password = "correct horse battery staple";
    let user = stores
        .user()
        .create(NewUser {
            email: "pw@example.com".into(),
            password: password.into(),
        })
        .await?;

    assert!(
        stores
            .user()
            .verify_password("pw@example.com", password)
            .await?
            .is_some()
    );
    assert!(
        stores
            .user()
            .verify_password("pw@example.com", "wrong password")
            .await?
            .is_none()
    );

    let row = users::Entity::find_by_id(user.id)
        .one(&db.conn)
        .await?
        .expect("user was just created");
    let stored_hash = row.password_hash.expect("password-based user has a hash");
    assert_ne!(stored_hash, password);
    assert!(!stored_hash.contains(password));

    db.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn a_session_round_trips_and_logout_invalidates_it() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "session@example.com".into(),
            password: "whatever12345".into(),
        })
        .await?;

    let cookie = auth::create_session(&db.conn, user.id, std::time::Duration::from_secs(3600))
        .await
        .expect("creating a session");
    let caller = auth::resolve_session(&db.conn, &cookie)
        .await?
        .expect("freshly created session resolves");
    assert_eq!(caller.id, user.id);
    assert_eq!(caller.kind, api2mcp::server::identity::CallerKind::Session);

    auth::delete_session(&db.conn, &cookie).await?;
    assert!(auth::resolve_session(&db.conn, &cookie).await?.is_none());

    db.teardown().await?;
    Ok(())
}

/// No database needed — `validate_next` is pure. Included here (rather than only as a
/// `#[cfg(test)]` unit test in `server::login`) because it is one of this chunk's named
/// minimum test cases.
#[test]
fn next_param_rejects_open_redirects_and_accepts_a_same_site_path() {
    assert_eq!(login::validate_next(Some("//evil.com")), None);
    assert_eq!(login::validate_next(Some("https://evil.com")), None);
    assert_eq!(login::validate_next(Some("/runs")), Some("/runs"));
}
