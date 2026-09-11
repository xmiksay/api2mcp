//! `api2mcp token mint|list|revoke` — service-token lifecycle management, run directly
//! against the database (there is no admin HTTP API yet; that's chunk C14). `mint` is the
//! only place in the CLI allowed to print a plaintext token — `list` never prints anything
//! secret, since [`crate::store::ServiceTokenRecord`] has nowhere to put one.
//!
//! Single-tenant single-admin for now (see `m0007_seed_admin`): there is no `token`/`user`
//! flag combination that creates a *second* user, so `mint`/`list` default to the sole
//! existing user and only need `--owner <email>` once that stops being true.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::cli::TokenAction;
use crate::config::Config;
use crate::db;
use crate::server::identity::{SCOPE_ADMIN, SCOPE_MCP};
use crate::store::{StoreError, Stores, UserRecord};

pub async fn run(action: TokenAction) -> Result<()> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;
    let stores = Stores::new(conn);

    match action {
        TokenAction::Mint {
            label,
            scope,
            owner,
        } => mint(&stores, label, scope, owner).await,
        TokenAction::List { owner } => list(&stores, owner).await,
        TokenAction::Revoke { id } => revoke(&stores, &id).await,
    }
}

async fn mint(stores: &Stores, label: String, scope: String, owner: Option<String>) -> Result<()> {
    let scopes = parse_scopes(&scope)?;
    let owner = resolve_owner(stores, owner.as_deref()).await?;
    let minted = stores
        .service_token()
        .mint(owner.id, label, scopes, None)
        .await
        .context("minting service token")?;

    println!("token minted — this plaintext is shown once and cannot be recovered:");
    println!();
    println!("  {}", minted.plaintext);
    println!();
    println!("id:     {}", minted.record.id);
    println!("owner:  {}", owner.email);
    println!("label:  {}", minted.record.label);
    println!("scopes: {}", minted.record.scopes.join(","));
    Ok(())
}

async fn list(stores: &Stores, owner: Option<String>) -> Result<()> {
    let owner = resolve_owner(stores, owner.as_deref()).await?;
    let tokens = stores
        .service_token()
        .list_for_owner(owner.id)
        .await
        .context("listing service tokens")?;

    if tokens.is_empty() {
        println!("no service tokens for {}", owner.email);
        return Ok(());
    }

    println!(
        "{:<36}  {:<8}  {:<20}  {:<12}  {:<8}  created_at",
        "id", "prefix", "label", "scopes", "status"
    );
    for t in tokens {
        let status = if t.revoked_at.is_some() {
            "revoked"
        } else if t.expires_at.is_some_and(|e| e <= Utc::now()) {
            "expired"
        } else {
            "active"
        };
        println!(
            "{:<36}  {:<8}  {:<20}  {:<12}  {:<8}  {}",
            t.id,
            t.token_prefix,
            t.label,
            t.scopes.join(","),
            status,
            t.created_at.to_rfc3339(),
        );
    }
    Ok(())
}

async fn revoke(stores: &Stores, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).with_context(|| format!("{id:?} is not a valid token id"))?;
    match stores.service_token().revoke(uuid).await {
        Ok(()) => {
            println!("token {uuid} revoked");
            Ok(())
        }
        Err(StoreError::NotFound) => bail!("no such token: {uuid}"),
        Err(e) => Err(e.into()),
    }
}

/// Resolves `--owner <email>` when given, else falls back to the sole user account.
/// Errors (rather than guessing) when there is more than one, or none at all.
async fn resolve_owner(stores: &Stores, owner_email: Option<&str>) -> Result<UserRecord> {
    if let Some(email) = owner_email {
        return stores
            .user()
            .get_by_email(email)
            .await
            .context("looking up owner")?
            .ok_or_else(|| anyhow::anyhow!("no such user: {email:?}"));
    }
    let mut users = stores.user().list().await.context("listing users")?;
    match users.len() {
        0 => bail!(
            "no users exist yet — set A2M_ADMIN_EMAIL/A2M_ADMIN_PASSWORD before running \
             migrations, then retry"
        ),
        1 => Ok(users.remove(0)),
        _ => bail!("more than one user exists — pass --owner <email> to disambiguate"),
    }
}

/// Parses a comma-separated `--scope` value into the two-value set [`SCOPE_MCP`]/
/// [`SCOPE_ADMIN`] — see `server::identity`'s module doc for why there are only two.
fn parse_scopes(raw: &str) -> Result<Vec<String>> {
    let mut scopes: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    scopes.sort();
    scopes.dedup();

    if scopes.is_empty() {
        bail!("--scope must name at least one scope");
    }
    for s in &scopes {
        if s != SCOPE_MCP && s != SCOPE_ADMIN {
            bail!("unknown scope {s:?}: valid scopes are \"mcp\" and \"admin\"");
        }
    }
    Ok(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scopes_accepts_a_single_known_scope() {
        assert_eq!(parse_scopes("mcp").unwrap(), vec!["mcp"]);
    }

    #[test]
    fn parse_scopes_dedups_and_sorts_a_comma_list() {
        assert_eq!(
            parse_scopes("admin, mcp,admin").unwrap(),
            vec!["admin", "mcp"]
        );
    }

    #[test]
    fn parse_scopes_rejects_an_unknown_scope() {
        assert!(parse_scopes("mcp,superuser").is_err());
    }

    #[test]
    fn parse_scopes_rejects_empty_input() {
        assert!(parse_scopes("").is_err());
        assert!(parse_scopes(" , ").is_err());
    }
}
