//! `UserStore::find_or_create_by_oidc`'s claim step: a verified-email OIDC sign-in binds
//! `(issuer, subject)` to an existing *unlinked* row (a pending `--oidc-only` invite, or a
//! password account) instead of creating a duplicate. Split out of `tests/store_oidc_identity.rs`
//! to keep that file under the workspace's 400-line cap — same kind of test, same scratch
//! Postgres harness (skipped when `TEST_DATABASE_URL` is unset, see `tests/common/mod.rs`).

mod common;

use anyhow::Result;
use common::ScratchDb;

use api2mcp::store::{NewUser, StoreError, Stores};

/// The task's headline fix: a pending `--oidc-only` row is inert scaffolding until someone
/// actually signs in for it. A verified-email match claims it — binds `(issuer, subject)` — and
/// every sign-in after that resolves purely by identity, same as any other linked account.
#[tokio::test]
async fn a_pending_oidc_only_account_is_claimed_on_first_verified_sign_in() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let pending = users.create_pending_oidc("invitee@example.com").await?;

    let claimed = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-invitee",
            "invitee@example.com",
            true,
        )
        .await?;
    assert_eq!(
        claimed.id, pending.id,
        "claims the pending row, not a new one"
    );
    assert_eq!(
        claimed.oidc_issuer.as_deref(),
        Some("https://idp.example.com")
    );
    assert_eq!(claimed.oidc_subject.as_deref(), Some("sub-invitee"));

    // A second sign-in resolves the now-linked row by (issuer, subject) alone — `email_verified`
    // no longer matters, exactly like any other already-linked account.
    let second = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-invitee",
            "invitee@example.com",
            false,
        )
        .await?;
    assert_eq!(second.id, pending.id);

    db.teardown().await
}

/// The other named case: an existing *password* account, not just a pending invite, is claimed
/// the same way — and claiming only ever touches the OIDC columns, so the password keeps
/// working after the account also gains a linked identity.
#[tokio::test]
async fn a_password_account_with_a_matching_verified_email_is_claimed_and_keeps_its_password()
-> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let password_user = users
        .create(NewUser {
            email: "claim-me@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    let claimed = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-claim-me",
            "claim-me@example.com",
            true,
        )
        .await?;
    assert_eq!(claimed.id, password_user.id);
    assert!(
        claimed.has_password,
        "claiming must not touch password_hash"
    );
    assert_eq!(claimed.oidc_subject.as_deref(), Some("sub-claim-me"));

    let authenticated = users
        .verify_password("claim-me@example.com", "correct horse battery staple")
        .await?
        .expect("the original password still authenticates after the claim");
    assert_eq!(authenticated.id, password_user.id);

    db.teardown().await
}

/// An unverified (or entirely absent — [`api2mcp::server::oidc::fetch_identity`] maps that to
/// `false` before it ever reaches the store) email assertion must never claim a matching row: an
/// email is just a self-asserted string without verification, and claiming on it is exactly the
/// account-takeover shape the task warns about. Since `ux_users_email` is global, the safe
/// refusal surfaces as a clean conflict rather than a duplicate — and the original row is left
/// completely untouched.
#[tokio::test]
async fn an_unverified_email_does_not_claim_a_matching_unlinked_row() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let pending = users.create_pending_oidc("unverified@example.com").await?;

    let err = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-unverified",
            "unverified@example.com",
            false,
        )
        .await
        .expect_err("an unverified email must never claim a matching row");
    assert!(matches!(err, StoreError::Conflict(_)));

    // The pending row is exactly as it was — not claimed, not deleted, not duplicated.
    let still_pending = users
        .get_by_email("unverified@example.com")
        .await?
        .expect("the row still exists");
    assert_eq!(still_pending.id, pending.id);
    assert!(still_pending.oidc_issuer.is_none());
    assert!(!still_pending.has_password);

    db.teardown().await
}

/// A row already linked to a different `(issuer, subject)` must never be claimed by a matching
/// email, verified or not — that path stays exactly as `get_by_oidc_identity` already handles
/// it, unaffected by the claim step.
#[tokio::test]
async fn a_row_linked_to_a_different_identity_is_never_claimed_by_email() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let original = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-original",
            "shared@example.com",
            true,
        )
        .await?;

    let err = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-impostor",
            "shared@example.com",
            true,
        )
        .await
        .expect_err("a different identity must never be linked via a matching email");
    assert!(matches!(err, StoreError::Conflict(_)));

    // The original identity's link is completely unaffected.
    let still_original = users
        .get_by_oidc_identity("https://idp.example.com", "sub-original")
        .await?
        .expect("the original row still exists, still linked");
    assert_eq!(still_original.id, original.id);
    assert!(
        users
            .get_by_oidc_identity("https://idp.example.com", "sub-impostor")
            .await?
            .is_none(),
        "the impostor identity must never resolve to any row"
    );

    db.teardown().await
}

/// An email that matches no existing row at all — verified or not, it makes no difference —
/// always just creates a fresh account, same as before this chunk's claim step existed.
#[tokio::test]
async fn an_email_matching_nothing_creates_a_new_account_even_when_unverified() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let users = stores.user();

    let created = users
        .find_or_create_by_oidc(
            "https://idp.example.com",
            "sub-brand-new",
            "brand-new@example.com",
            false,
        )
        .await?;
    assert_eq!(created.email, "brand-new@example.com");
    assert_eq!(created.oidc_subject.as_deref(), Some("sub-brand-new"));
    assert!(!created.has_password);

    db.teardown().await
}
