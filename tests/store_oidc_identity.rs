//! `UserStore::find_or_create_by_oidc` integration tests, against a real scratch Postgres
//! (skipped when `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`). Split out of
//! `tests/store.rs` to keep that file under the workspace's 400-line cap; this is otherwise the
//! same kind of test as everything in there.

mod common;

use anyhow::Result;
use common::ScratchDb;

use api2mcp::store::Stores;

#[tokio::test]
async fn first_sign_in_creates_a_user_and_a_second_matches_by_issuer_and_subject() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let first = users
        .find_or_create_by_oidc("https://idp.example.com", "sub-123", "a@example.com", true)
        .await?;
    assert_eq!(first.email, "a@example.com");
    assert!(!first.has_password);
    assert_eq!(
        first.oidc_issuer.as_deref(),
        Some("https://idp.example.com")
    );
    assert_eq!(first.oidc_subject.as_deref(), Some("sub-123"));

    // Same (issuer, subject), same email: resolves to the identical row, doesn't duplicate it.
    let second = users
        .find_or_create_by_oidc("https://idp.example.com", "sub-123", "a@example.com", true)
        .await?;
    assert_eq!(second.id, first.id);

    db.teardown().await
}

/// The task's own named case: a provider is free to let a user change their email, and a
/// changed email must still resolve to the *same* user — matching is `(issuer, subject)` only,
/// never `email` (see `UserStore::find_or_create_by_oidc`'s own doc for why: matching on email
/// would let a changed email hijack another account).
#[tokio::test]
async fn a_changed_email_on_a_second_sign_in_still_resolves_to_the_same_user() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let first = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-456",
            "old@example.com",
            true,
        )
        .await?;

    let second = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-456",
            "new@example.com",
            true,
        )
        .await?;
    assert_eq!(
        second.id, first.id,
        "must resolve to the same user, not a new one"
    );
    assert_eq!(
        second.email, "new@example.com",
        "the stored email is refreshed"
    );

    // And the stale email doesn't linger as a second lookup path.
    assert!(users.get_by_email("old@example.com").await?.is_none());

    db.teardown().await
}

/// Two different providers may legitimately hand out the same `sub` value to different people —
/// the pair is what must be unique, not either half alone.
#[tokio::test]
async fn the_same_subject_from_two_different_issuers_are_different_users() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let a = users
        .find_or_create_by_oidc(
            "https://idp-a.example.com",
            "shared-sub",
            "a@example.com",
            true,
        )
        .await?;
    let b = users
        .find_or_create_by_oidc(
            "https://idp-b.example.com",
            "shared-sub",
            "b@example.com",
            true,
        )
        .await?;
    assert_ne!(a.id, b.id);

    db.teardown().await
}

/// A local-password account and an OIDC-only account are both just rows in the same table with
/// no privilege distinction between them (Decision 1) — this pins the `has_password`/
/// `oidc_issuer` shape `UserStore::create` and `find_or_create_by_oidc` each produce.
#[tokio::test]
async fn a_password_user_and_an_oidc_user_carry_the_expected_login_shape() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let password_user = users
        .create(api2mcp::store::NewUser {
            email: "pw-user@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    assert!(password_user.has_password);
    assert!(password_user.oidc_issuer.is_none());

    let oidc_user = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-789",
            "oidc-user@example.com",
            true,
        )
        .await?;
    assert!(!oidc_user.has_password);
    assert!(oidc_user.oidc_issuer.is_some());

    db.teardown().await
}

/// `api2mcp user add --oidc-only` (`UserStore::create_pending_oidc`): a real row with a real
/// email, no password, and no linked identity yet — the password-login path must refuse it, not
/// panic on the `NULL` hash.
#[tokio::test]
async fn a_pending_oidc_only_user_has_no_password_and_cannot_password_login() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let pending = users.create_pending_oidc("pending@example.com").await?;
    assert!(!pending.has_password);
    assert!(pending.oidc_issuer.is_none());
    assert!(pending.oidc_subject.is_none());

    assert!(
        users
            .verify_password("pending@example.com", "anything-at-all")
            .await?
            .is_none()
    );

    db.teardown().await
}

/// A duplicate email is a clean typed [`api2mcp::store::StoreError::Conflict`], the same
/// shape `UserStore::create` already produces — never a raw unique-constraint violation
/// reaching the caller.
#[tokio::test]
async fn create_pending_oidc_rejects_a_duplicate_email() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    users.create_pending_oidc("dup@example.com").await?;
    let err = users
        .create_pending_oidc("dup@example.com")
        .await
        .expect_err("a second call with the same email must fail");
    assert!(matches!(err, api2mcp::store::StoreError::Conflict(_)));

    // Also a conflict against an *existing password account's* email, in either direction —
    // `ux_users_email` is global regardless of login method.
    users
        .create(api2mcp::store::NewUser {
            email: "mixed@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let err = users
        .create_pending_oidc("mixed@example.com")
        .await
        .expect_err("must conflict with an existing password account's email too");
    assert!(matches!(err, api2mcp::store::StoreError::Conflict(_)));

    db.teardown().await
}
