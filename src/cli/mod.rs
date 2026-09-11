//! The `api2mcp` command line.
//!
//! Subcommands are the local-process entry points for everything an agent must never be able to
//! do: importing definitions, minting tokens, binding an auth provider to an origin (I5).

use anyhow::{Result, bail};
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
    /// Run the HTTP server: MCP data plane, OAuth AS, read-only API and the embedded SPA.
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
        /// Endpoint to resolve the tool against. Defaults to `cfg.default_endpoint`.
        #[arg(long)]
        endpoint: Option<String>,
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
    },
    /// Import a YAML pack. Last write wins; definitions are not versioned.
    Import {
        path: std::path::PathBuf,
        /// Validate and report without writing.
        #[arg(long)]
        dry_run: bool,
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
        /// Endpoint to resolve the tool against. Defaults to `cfg.default_endpoint`.
        #[arg(long)]
        endpoint: Option<String>,
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
        } => call::run(&name, &args, raw, endpoint.as_deref()).await,
        Command::Script { action } => match action {
            ScriptAction::Run {
                name,
                args,
                endpoint,
            } => script::run(&name, &args, endpoint.as_deref()).await,
        },
        Command::Export { endpoint } => pack::export(&endpoint).await,
        Command::Import { path, dry_run } => pack::import(&path, dry_run).await,
        Command::Token { action } => token::run(action).await,
        Command::User { action } => user::run(action).await,
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn args_parse_as_json_with_a_string_fallback() {
        let got = parse_args(&["n=3".into(), "s=hello".into(), "b=true".into()]).unwrap();
        assert_eq!(got["n"], json!(3));
        assert_eq!(got["s"], json!("hello"));
        assert_eq!(got["b"], json!(true));
    }

    #[test]
    fn a_value_containing_equals_keeps_its_tail() {
        let got = parse_args(&["q=a=b".into()]).unwrap();
        assert_eq!(got["q"], json!("a=b"));
    }

    #[test]
    fn retyping_narrows_a_number_onto_a_string_param() {
        use crate::model::{Param, ParamLocation, ParamType};
        let p = Param {
            name: "id".into(),
            location: ParamLocation::Path,
            ty: ParamType::String,
            required: true,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        };
        let mut args = parse_args(&["id=3".into()]).expect("parses");
        assert_eq!(args["id"], json!(3), "parse_args guesses a number");
        retype_args_for_params(&[p], &mut args);
        assert_eq!(args["id"], json!("3"), "retyped to the declared string");
    }

    #[test]
    fn retyping_leaves_a_non_string_param_alone() {
        use crate::model::{Param, ParamLocation, ParamType};
        let p = Param {
            name: "limit".into(),
            location: ParamLocation::Query,
            ty: ParamType::Integer,
            required: false,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        };
        let mut args = parse_args(&["limit=10".into()]).expect("parses");
        retype_args_for_params(&[p], &mut args);
        assert_eq!(
            args["limit"],
            json!(10),
            "an integer param keeps its number"
        );
    }

    #[test]
    fn a_missing_equals_is_an_error() {
        assert!(parse_args(&["oops".into()]).is_err());
    }
}
