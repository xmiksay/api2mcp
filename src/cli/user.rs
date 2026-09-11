//! `api2mcp user list|passwd` — admin account management. There is deliberately no
//! `user add`: the only account this crate ever creates is the one migration
//! `m0007_seed_admin` seeds from `A2M_ADMIN_EMAIL`/`A2M_ADMIN_PASSWORD`, so `passwd` (change
//! a password) and `list` (see who exists) are all today's single-tenant deployment needs.

use std::io::{self, Write as _};

use anyhow::{Context, Result, bail};
use sea_orm::DatabaseConnection;

use crate::cli::UserAction;
use crate::config::Config;
use crate::db;
use crate::store::{Stores, UserStore};

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
    let new_password = read_new_password()?;
    validate_password(&new_password)?;
    UserStore::new(conn.clone())
        .set_password(email, &new_password)
        .await
        .with_context(|| format!("setting password for {email:?}"))?;
    println!("password updated for {email}");
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
}
