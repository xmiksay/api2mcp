//! The `api2mcp` command line.
//!
//! Subcommands are the local-process entry points for everything an agent must never be able to
//! do: importing definitions, minting tokens, binding an auth provider to an origin (I5).

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

pub mod migrate;

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
    },
    /// Execute a script.
    Script {
        name: String,
        #[arg(long = "arg", value_name = "KEY=VALUE")]
        args: Vec<String>,
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
    /// Manage admin users.
    User {
        #[command(subcommand)]
        action: UserAction,
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
        #[arg(long, default_value = "mcp")]
        scope: String,
    },
    List,
    Revoke {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserAction {
    List,
    Passwd { email: String },
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve => bail!("serve: not implemented yet (chunk C11)"),
        Command::Migrate { action } => migrate::run(action).await,
        Command::Call { .. } => bail!("call: not implemented yet (chunk C8)"),
        Command::Script { .. } => bail!("script: not implemented yet (chunk C9)"),
        Command::Export { .. } | Command::Import { .. } => {
            bail!("pack: not implemented yet (chunk C13)")
        }
        Command::Token { .. } | Command::User { .. } => {
            bail!("token/user: not implemented yet (chunk C10)")
        }
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
    fn a_missing_equals_is_an_error() {
        assert!(parse_args(&["oops".into()]).is_err());
    }
}
