//! `api2mcp token mint|list|revoke` — service-token lifecycle management, run directly
//! against the database (there is no admin HTTP API yet; that's chunk C14). `mint` is the
//! only place in the CLI allowed to print a plaintext token — `list` never prints anything
//! secret, since [`crate::store::ServiceTokenRecord`] has nowhere to put one.
//!
//! Single-tenant for now (see `m0007_seed_first_user`): there is no `token`/`user` flag
//! combination that creates a *second* user, so `mint`/`list` default to the sole existing
//! user and only need `--owner <email>` once that stops being true.
//!
//! There is no `--scope` flag, and no `scopes` column to display: the admin/mcp split a
//! service token's scope list used to choose between is gone (see `server::identity`'s module
//! doc), and with it every reason a token would ever need one. A resolved, unrevoked, unexpired
//! token may call tools over `/mcp` — that's the whole rule now.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::cli::TokenAction;
use crate::config::Config;
use crate::db;
use crate::store::{StoreError, Stores, UserRecord};

pub async fn run(action: TokenAction) -> Result<()> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;
    let stores = Stores::new(conn);

    match action {
        TokenAction::Mint { label, owner } => mint(&stores, label, owner).await,
        TokenAction::List { owner } => list(&stores, owner).await,
        TokenAction::Revoke { id } => revoke(&stores, &id).await,
    }
}

async fn mint(stores: &Stores, label: String, owner: Option<String>) -> Result<()> {
    let owner = resolve_owner(stores, owner.as_deref()).await?;
    let minted = stores
        .service_token()
        .mint(owner.id, label, None)
        .await
        .context("minting service token")?;

    println!("token minted — this plaintext is shown once and cannot be recovered:");
    println!();
    println!("  {}", minted.plaintext);
    println!();
    println!("id:    {}", minted.record.id);
    println!("owner: {}", owner.email);
    println!("label: {}", minted.record.label);
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
        "{:<36}  {:<8}  {:<20}  {:<8}  created_at",
        "id", "prefix", "label", "status"
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
            "{:<36}  {:<8}  {:<20}  {:<8}  {}",
            t.id,
            t.token_prefix,
            t.label,
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
            "no users exist yet — set A2M_SEED_EMAIL/A2M_SEED_PASSWORD before running \
             migrations, then retry"
        ),
        1 => Ok(users.remove(0)),
        _ => bail!("more than one user exists — pass --owner <email> to disambiguate"),
    }
}

// No unit tests: every function here is a thin, database-backed delegation to `store::` methods
// (already unit- and integration-tested on their own) plus `println!` formatting. Covered
// end-to-end by `tests/store.rs`'s service-token round trip and `tests/auth.rs`.
