//! The `api2mcp` command line.
//!
//! Subcommands are the local-process entry points for everything an agent must never be able to
//! do: importing definitions, minting tokens, binding an auth provider to an origin (I5).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

pub mod call;
pub mod migrate;
pub mod pack;
pub mod script;
pub mod serve;
pub mod token;
pub mod user;

#[derive(Parser, Debug)]
#[command(name = "api2mcp", version = crate::version::LONG_VERSION, about = "MCP Tool Factory")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the HTTP server: MCP data plane, OAuth AS, read-write admin JSON API and the
    /// embedded SPA.
    Serve,
    /// Apply, roll back or inspect database migrations.
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
    /// Execute a single api_call and print both the raw and the projected response.
    Call {
        /// api_call slug.
        name: String,
        /// Repeatable `key=value` argument.
        #[arg(long = "arg", value_name = "KEY=VALUE")]
        args: Vec<String>,
        /// Also print the unprojected upstream response.
        #[arg(long)]
        raw: bool,
        /// Endpoint to resolve the tool against. Defaults to `"default"`.
        #[arg(long)]
        endpoint: Option<String>,
        /// Email of the user whose definitions to resolve against. Defaults to the sole user
        /// account when omitted — only needed once more than one user exists.
        #[arg(long)]
        user: Option<String>,
    },
    /// Execute a script.
    Script {
        #[command(subcommand)]
        action: ScriptAction,
    },
    /// Write a YAML pack for an endpoint to stdout.
    Export {
        #[arg(long)]
        endpoint: String,
        /// Email of the user whose definitions to export. Defaults to the sole user account
        /// when omitted — only needed once more than one user exists.
        #[arg(long)]
        user: Option<String>,
    },
    /// Import a YAML pack. Last write wins; definitions are not versioned.
    Import {
        path: std::path::PathBuf,
        /// Validate and report without writing.
        #[arg(long)]
        dry_run: bool,
        /// Email of the user who will own everything this pack creates. Defaults to the sole
        /// user account when omitted — only needed once more than one user exists.
        #[arg(long)]
        user: Option<String>,
    },
    /// Manage service tokens.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Manage user accounts.
    User {
        #[command(subcommand)]
        action: UserAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScriptAction {
    /// Run a script and print its result.
    Run {
        /// script slug.
        name: String,
        /// Repeatable `key=value` argument.
        #[arg(long = "arg", value_name = "KEY=VALUE")]
        args: Vec<String>,
        /// Endpoint to resolve the tool against. Defaults to `"default"`.
        #[arg(long)]
        endpoint: Option<String>,
        /// Email of the user whose definitions to resolve against. Defaults to the sole user
        /// account when omitted — only needed once more than one user exists.
        #[arg(long)]
        user: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum MigrateAction {
    Up,
    Down { steps: Option<u32> },
    Status,
}

#[derive(Subcommand, Debug)]
pub enum TokenAction {
    /// Mint a token. The plaintext is printed once and never stored.
    Mint {
        #[arg(long)]
        label: String,
        /// Email of the token's owner. Defaults to the sole user account when omitted —
        /// only needed once more than one user exists.
        #[arg(long)]
        owner: Option<String>,
        /// Restrict the token to this endpoint slug; repeatable. Omit entirely to mint an
        /// unrestricted token that can reach every endpoint (the default, equivalent to a
        /// GitHub classic PAT).
        #[arg(long = "endpoint")]
        endpoints: Vec<String>,
        /// Expire the token after this many days. Omit for a token that never expires.
        #[arg(long)]
        expires_in_days: Option<i64>,
        /// Grant this token access to the control plane (bare `POST /mcp`, the definition-
        /// authoring factory) — an explicit opt-in, off by default, unrelated to `--endpoint`
        /// (which gates `/mcp/{slug}` instead).
        #[arg(long)]
        control_plane: bool,
    },
    List {
        #[arg(long)]
        owner: Option<String>,
    },
    Revoke {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserAction {
    List,
    /// Create a user. Password from `A2M_CLI_PASSWORD` or an interactive prompt (same as
    /// `passwd`), unless `--oidc-only`.
    Add {
        email: String,
        /// No password: the account is created with `password_hash` left `null`, awaiting a
        /// future sign-in to attach an OIDC identity. See `cli::user`'s module doc for the
        /// current limits of that (there is no linking step yet).
        #[arg(long)]
        oidc_only: bool,
    },
    Passwd {
        email: String,
    },
    /// Delete a user. Refused on the last remaining user unless `--force` — a deployment with
    /// zero users has no way back in except editing the database by hand.
    Delete {
        email: String,
        #[arg(long)]
        force: bool,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve => serve::run().await,
        Command::Migrate { action } => migrate::run(action).await,
        Command::Call {
            name,
            args,
            raw,
            endpoint,
            user,
        } => call::run(&name, &args, raw, endpoint.as_deref(), user.as_deref()).await,
        Command::Script { action } => match action {
            ScriptAction::Run {
                name,
                args,
                endpoint,
                user,
            } => script::run(&name, &args, endpoint.as_deref(), user.as_deref()).await,
        },
        Command::Export { endpoint, user } => pack::export(&endpoint, user.as_deref()).await,
        Command::Import {
            path,
            dry_run,
            user,
        } => pack::import(&path, dry_run, user.as_deref()).await,
        Command::Token { action } => token::run(action).await,
        Command::User { action } => user::run(action).await,
    }
}

/// Resolves `--user <email>` when given, else falls back to the sole user account — the shared
/// rule `call`/`script run`/`import`/`export` all need, since none of them have a session cookie
/// to derive an owner from. A single-person self-hosted box never has to pass `--user`; a
/// multi-user one must not have this guess, so more than one user with no `--user` is a hard
/// error rather than picking one arbitrarily. Mirrors `cli::token::resolve_owner`, which predates
/// this and already had to solve the identical problem for `token mint`/`list`.
pub(crate) async fn resolve_user(
    stores: &crate::store::Stores,
    email: Option<&str>,
) -> Result<crate::store::UserRecord> {
    if let Some(email) = email {
        return stores
            .user()
            .get_by_email(email)
            .await
            .context("looking up --user")?
            .ok_or_else(|| anyhow::anyhow!("no such user: {email:?}"));
    }
    let mut users = stores.user().list().await.context("listing users")?;
    match users.len() {
        0 => bail!(
            "no users exist yet — set A2M_SEED_EMAIL/A2M_SEED_PASSWORD before running \
             migrations, then retry"
        ),
        1 => Ok(users.remove(0)),
        _ => bail!("more than one user exists — pass --user <email> to disambiguate"),
    }
}

/// Parse repeated `--arg key=value` into a JSON object. Values parse as JSON when they can and
/// fall back to a string, so `--arg n=3` is a number but `--arg s=hello` is not an error.
pub fn parse_args(pairs: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut out = serde_json::Map::new();
    for p in pairs {
        let Some((k, v)) = p.split_once('=') else {
            bail!("--arg must be KEY=VALUE, got {p:?}");
        };
        let value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into()));
        out.insert(k.to_string(), value);
    }
    Ok(out)
}

/// Re-reads `--arg` values against the parameters they will actually bind to.
///
/// [`parse_args`] has to guess: it tries JSON so `--arg limit=10` is a number, and falls back to a
/// string. That guess is wrong exactly when a `string`-typed parameter is given a bare numeric or
/// boolean-looking value — `--arg id=3` becomes the number `3` and is then correctly rejected by
/// `schema::bind_args`, which is baffling from a shell where everything is text anyway.
///
/// The narrowing is deliberately one-directional and lives here rather than in `schema::coerce`.
/// A model sending `3` for a `string` parameter should still be told so: it had a typed schema in
/// front of it and ignored it. A person typing at a shell had no such thing.
pub fn retype_args_for_params(
    params: &[crate::model::Param],
    args: &mut serde_json::Map<String, serde_json::Value>,
) {
    for p in params {
        if p.ty != crate::model::ParamType::String {
            continue;
        }
        let Some(value) = args.get_mut(&p.name) else {
            continue;
        };
        let restated = match value {
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        };
        if let Some(text) = restated {
            *value = serde_json::Value::String(text);
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
