//! `api2mcp user list|passwd` — admin account management. There is deliberately no
//! `user add`: the only account this crate ever creates is the one migration
//! `m0007_seed_admin` seeds from `A2M_ADMIN_EMAIL`/`A2M_ADMIN_PASSWORD`, so `passwd` (change
//! a password) and `list` (see who exists) are all today's single-tenant deployment needs.
//!
//! `store::user` has no `set_password` — only `create`/`verify_password`/`list`/`delete` —
//! and this chunk may not add one (`src/store/` is out of scope here). `passwd` therefore
//! updates `entity::users` directly via `sea_orm`, the same narrow, documented exception
//! `server::auth` takes for sessions. A follow-up chunk should add a proper
//! `UserStore::set_password` and fold this back in.

use std::io::{self, Write as _};

use anyhow::{Context, Result, bail};
use argon2::Argon2;
use argon2::password_hash::PasswordHasher;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set,
};

use crate::cli::UserAction;
use crate::config::Config;
use crate::db;
use crate::entity::users;
use crate::store::Stores;

pub async fn run(action: UserAction) -> Result<()> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;

    match action {
        UserAction::List => list(&conn).await,
        UserAction::Passwd { email } => passwd(&conn, &email).await,
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
    println!("{:<36}  {:<8}  {:<30}  created_at", "id", "admin", "email");
    for u in users {
        println!(
            "{:<36}  {:<8}  {:<30}  {}",
            u.id,
            u.is_admin,
            u.email,
            u.created_at.to_rfc3339()
        );
    }
    Ok(())
}

async fn passwd(conn: &DatabaseConnection, email: &str) -> Result<()> {
    let row = users::Entity::find()
        .filter(users::Column::Email.eq(email))
        .one(conn)
        .await
        .context("looking up user")?
        .ok_or_else(|| anyhow::anyhow!("no such user: {email:?}"))?;

    let new_password = read_new_password()?;
    validate_password(&new_password)?;
    let hash = hash_password(&new_password)?;

    let mut active = row.into_active_model();
    active.password_hash = Set(hash);
    active.update(conn).await.context("updating password")?;

    println!("password updated for {email}");
    Ok(())
}

/// A password never appears on the command line (no `--password` flag) — `A2M_CLI_PASSWORD`
/// for scripted/test use, else an interactive stdin prompt. The prompt is not
/// terminal-masked: this chunk has no dependency that does that (`rpassword` is not in
/// `Cargo.toml`, which is out of scope here), so the value is echoed to the terminal. Worth
/// revisiting alongside the `store::user::set_password` follow-up noted above.
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

fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
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
    fn hash_password_never_returns_the_plaintext() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert_ne!(hash, "correct horse battery staple");
        assert!(hash.starts_with("$argon2"));
    }
}
