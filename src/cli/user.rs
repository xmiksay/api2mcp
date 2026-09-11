//! `api2mcp user list|add|passwd|delete` — user account management. There is no admin/non-admin
//! distinction (every account can read and write every definition); this exists because the
//! system is genuinely multi-user now (a follow-up chunk gives every definition an `owner_id`),
//! and running `api2mcp` locally is the only way to onboard a second person on a self-hosted
//! box — there is no self-service signup route (see `server::login`'s own doc on why login is
//! server-rendered, not an admin-API concern).
//!
//! `add --oidc-only` pre-provisions an account with no password
//! (`store::user::create_pending_oidc`) for someone who will sign in externally. That account
//! has no working login until that first sign-in happens:
//! `store::user::find_or_create_by_oidc` claims the row — binding `(issuer, subject)` to it —
//! the first time an OIDC sign-in presents a `email_verified: true` email matching it exactly.
//! This is not the account-takeover shape a stale-email match would be: the row it claims has
//! no identity bound to it yet, so there is nothing to take over (see that function's own doc
//! for the full reasoning). Until that first sign-in happens, the row stays inert —
//! `store::user::UserStore::verify_password` refuses it (no password hash), and it is never a
//! candidate for a *linked* account's email refresh.

use std::io::{self, Write as _};

use anyhow::{Context, Result, bail};
use sea_orm::DatabaseConnection;

use crate::cli::UserAction;
use crate::config::Config;
use crate::db;
use crate::store::{NewUser, Stores, UserStore};

pub async fn run(action: UserAction) -> Result<()> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;

    match action {
        UserAction::List => list(&conn).await,
        UserAction::Add { email, oidc_only } => add(&conn, &email, oidc_only).await,
        UserAction::Passwd { email } => passwd(&conn, &email).await,
        UserAction::Delete { email, force } => delete(&conn, &email, force).await,
    }
}

async fn list(conn: &DatabaseConnection) -> Result<()> {
    let users = Stores::new(conn.clone())
        .user()
        .list()
        .await
        .context("listing users")?;
    if users.is_empty() {
        println!("no users");
        return Ok(());
    }
    println!("{:<36}  {:<10}  {:<30}  created_at", "id", "login", "email");
    for u in users {
        let login = match (u.has_password, &u.oidc_issuer) {
            (true, _) => "password",
            (false, Some(_)) => "oidc",
            // A pending `add --oidc-only` row (see this module's own doc), not an impossible
            // state — `ck_users_oidc_pair` no longer requires at least one login method.
            (false, None) => "pending",
        };
        println!(
            "{:<36}  {:<10}  {:<30}  {}",
            u.id,
            login,
            u.email,
            u.created_at.to_rfc3339()
        );
    }
    Ok(())
}

/// Resolves the password (env var or prompt) and dispatches to whichever of
/// [`add_with_password`]/[`add_oidc_only`] applies — split out so both can be unit-tested
/// directly against a scratch database without also exercising the stdin prompt.
async fn add(conn: &DatabaseConnection, email: &str, oidc_only: bool) -> Result<()> {
    if oidc_only {
        return add_oidc_only(conn, email).await;
    }
    let password = read_new_password()?;
    validate_password(&password)?;
    add_with_password(conn, email, &password).await
}

async fn add_with_password(conn: &DatabaseConnection, email: &str, password: &str) -> Result<()> {
    UserStore::new(conn.clone())
        .create(NewUser {
            email: email.to_owned(),
            password: password.to_owned(),
        })
        .await
        .with_context(|| format!("creating {email:?}"))?;
    println!("created {email}");
    Ok(())
}

async fn add_oidc_only(conn: &DatabaseConnection, email: &str) -> Result<()> {
    UserStore::new(conn.clone())
        .create_pending_oidc(email)
        .await
        .with_context(|| format!("creating {email:?}"))?;
    println!(
        "created {email} (oidc-only, no password set — has no working login until it is \
         linked to a sign-in)"
    );
    Ok(())
}

async fn passwd(conn: &DatabaseConnection, email: &str) -> Result<()> {
    let new_password = read_new_password()?;
    validate_password(&new_password)?;
    UserStore::new(conn.clone())
        .set_password(email, &new_password)
        .await
        .with_context(|| format!("setting password for {email:?}"))?;
    println!("password updated for {email}");
    Ok(())
}

async fn delete(conn: &DatabaseConnection, email: &str, force: bool) -> Result<()> {
    let store = UserStore::new(conn.clone());
    let existing = store
        .get_by_email(email)
        .await
        .context("looking up user")?
        .ok_or_else(|| anyhow::anyhow!("no such user: {email:?}"))?;
    let total = store.list().await.context("listing users")?.len();
    last_user_guard(total, force).with_context(|| format!("refusing to delete {email:?}"))?;
    store
        .delete(existing.id)
        .await
        .with_context(|| format!("deleting {email:?}"))?;
    println!("deleted {email}");
    Ok(())
}

/// Refuses to proceed when `total` is the last remaining user and `force` wasn't given — a
/// deployment with zero users has no way back in except editing the database by hand. Pure and
/// unit-tested on its own: `delete`'s only other logic is database I/O.
fn last_user_guard(total: usize, force: bool) -> Result<()> {
    if total <= 1 && !force {
        bail!(
            "this is the last remaining user — pass --force to delete it anyway (there would \
             then be no way to sign back in except editing the database by hand)"
        );
    }
    Ok(())
}

/// A password never appears on the command line (no `--password` flag) — `A2M_CLI_PASSWORD`
/// for scripted/test use, else an interactive stdin prompt. The prompt is not
/// terminal-masked: no dependency in the tree does that, so the value is echoed. Worth
/// revisiting if this command ever gets used outside a trusted local shell.
fn read_new_password() -> Result<String> {
    if let Ok(v) = std::env::var("A2M_CLI_PASSWORD")
        && !v.is_empty()
    {
        return Ok(v);
    }
    print!("new password: ");
    io::stdout().flush().context("flushing prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("reading password from stdin")?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn validate_password(password: &str) -> Result<()> {
    if password.len() < 8 {
        bail!("password must be at least 8 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_password_rejects_short_input() {
        assert!(validate_password("short").is_err());
    }

    #[test]
    fn validate_password_accepts_eight_or_more_chars() {
        assert!(validate_password("longenough").is_ok());
    }

    #[test]
    fn last_user_guard_refuses_the_last_user_without_force() {
        assert!(last_user_guard(1, false).is_err());
    }

    #[test]
    fn last_user_guard_allows_the_last_user_with_force() {
        assert!(last_user_guard(1, true).is_ok());
    }

    #[test]
    fn last_user_guard_allows_any_user_when_more_than_one_remains() {
        assert!(last_user_guard(2, false).is_ok());
    }

    #[test]
    fn last_user_guard_refuses_when_the_count_is_already_zero() {
        // Shouldn't happen in practice (the caller already found the user by email), but the
        // guard's own arithmetic must not treat "no users" as "plenty of users".
        assert!(last_user_guard(0, false).is_err());
    }

    // `add`/`delete` themselves are thin (env/prompt resolution, then a database call) — these
    // exercise that database-facing half directly, against a real scratch Postgres. See
    // `store::test_support`'s own doc for why this harness duplicates `tests/common::ScratchDb`
    // rather than sharing it (this file's tests are unit tests inside the library crate;
    // `tests/` is a separate crate that can't see `add`/`delete` at all, since they're private).
    mod db_tests {
        use super::*;
        use crate::store::UserStore;
        use crate::store::test_support::ScratchDb;

        #[tokio::test]
        async fn add_with_password_creates_a_user_that_can_then_authenticate() {
            let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            };
            add_with_password(
                &scratch.db,
                "add-pw@example.com",
                "correct horse battery staple",
            )
            .await
            .expect("add succeeds");

            let users = UserStore::new(scratch.db.clone());
            let authenticated = users
                .verify_password("add-pw@example.com", "correct horse battery staple")
                .await
                .expect("verify succeeds")
                .expect("the just-created user authenticates");
            assert_eq!(authenticated.email, "add-pw@example.com");

            scratch.teardown().await.expect("teardown");
        }

        #[tokio::test]
        async fn add_with_password_rejects_a_duplicate_email() {
            let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            };
            add_with_password(
                &scratch.db,
                "dup@example.com",
                "correct horse battery staple",
            )
            .await
            .expect("first add succeeds");
            let err = add_with_password(&scratch.db, "dup@example.com", "another password!")
                .await
                .expect_err("a duplicate email must fail");
            assert!(format!("{err:#}").contains("already exists"));

            scratch.teardown().await.expect("teardown");
        }

        #[tokio::test]
        async fn add_oidc_only_leaves_no_password_hash_and_password_login_is_refused() {
            let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            };
            add_oidc_only(&scratch.db, "oidc-only@example.com")
                .await
                .expect("add succeeds");

            let users = UserStore::new(scratch.db.clone());
            let created = users
                .get_by_email("oidc-only@example.com")
                .await
                .expect("lookup succeeds")
                .expect("the user exists");
            assert!(!created.has_password);
            assert!(
                users
                    .verify_password("oidc-only@example.com", "anything-at-all")
                    .await
                    .expect("verify succeeds")
                    .is_none()
            );

            scratch.teardown().await.expect("teardown");
        }

        #[tokio::test]
        async fn delete_removes_a_user() {
            let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            };
            // A second user, so the one under test isn't the last remaining one.
            add_with_password(
                &scratch.db,
                "keep@example.com",
                "correct horse battery staple",
            )
            .await
            .expect("seed succeeds");
            add_with_password(
                &scratch.db,
                "gone@example.com",
                "correct horse battery staple",
            )
            .await
            .expect("add succeeds");

            delete(&scratch.db, "gone@example.com", false)
                .await
                .expect("delete succeeds");

            let users = UserStore::new(scratch.db.clone());
            assert!(
                users
                    .get_by_email("gone@example.com")
                    .await
                    .expect("lookup succeeds")
                    .is_none()
            );

            scratch.teardown().await.expect("teardown");
        }

        #[tokio::test]
        async fn deleting_the_last_user_is_refused_without_force() {
            let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            };
            add_with_password(
                &scratch.db,
                "only-one@example.com",
                "correct horse battery staple",
            )
            .await
            .expect("add succeeds");

            let err = delete(&scratch.db, "only-one@example.com", false)
                .await
                .expect_err("deleting the last user must be refused");
            assert!(format!("{err:#}").contains("last remaining user"));

            // ...but proceeds with `--force`.
            delete(&scratch.db, "only-one@example.com", true)
                .await
                .expect("force delete succeeds");
            let users = UserStore::new(scratch.db.clone());
            assert!(users.list().await.expect("list succeeds").is_empty());

            scratch.teardown().await.expect("teardown");
        }
    }
}
