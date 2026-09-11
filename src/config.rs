//! Process configuration, resolved once at startup.
//!
//! Parsing is split into [`Config::from_env`] and [`Config::from_lookup`] so the whole layer is
//! unit-testable against a `HashMap` without mutating the process environment — `env::set_var`
//! in a test races every other test in the binary.

use anyhow::{Context, Result, bail};
use std::time::Duration;

/// Prefix for upstream credential env vars. An `auth_provider` row stores the *name* of one of
/// these, never a value; see [`crate::secret`].
pub const CREDENTIAL_PREFIX: &str = "A2M_CRED_";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
    pub host: String,
    pub port: u16,
    /// Public origin this server is reached at. It is the OAuth issuer and the base for the
    /// `.well-known` metadata documents, so a wrong value breaks discovery rather than
    /// degrading it gracefully.
    pub base_url: String,
    /// Endpoint slug that bare `POST /mcp` resolves to.
    pub default_endpoint: String,
    pub admin_email: Option<String>,
    pub admin_password: Option<String>,
    pub run_retention_days: u32,
    /// Let the SSRF guard accept loopback upstreams. False everywhere except integration
    /// tests, whose fixture server is a real listener on 127.0.0.1.
    pub allow_loopback_upstream: bool,
    pub session_ttl: Duration,
    pub max_request_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let host = get("A2M_HOST").unwrap_or_else(|| "127.0.0.1".into());
        let port = parse_or(get("A2M_PORT"), 8080, "A2M_PORT")?;
        let cfg = Self {
            database_url: get("DATABASE_URL").context("DATABASE_URL must be set")?,
            base_url: get("A2M_BASE_URL")
                .map(|v| v.trim_end_matches('/').to_string())
                .unwrap_or_else(|| format!("http://{host}:{port}")),
            host,
            port,
            default_endpoint: get("A2M_DEFAULT_ENDPOINT").unwrap_or_else(|| "default".into()),
            admin_email: get("A2M_ADMIN_EMAIL"),
            admin_password: get("A2M_ADMIN_PASSWORD"),
            run_retention_days: parse_or(
                get("A2M_RUN_RETENTION_DAYS"),
                30,
                "A2M_RUN_RETENTION_DAYS",
            )?,
            allow_loopback_upstream: parse_bool(get("A2M_ALLOW_LOOPBACK_UPSTREAM"))?,
            session_ttl: Duration::from_secs(
                60 * 60
                    * parse_or::<u64>(get("A2M_SESSION_TTL_HOURS"), 720, "A2M_SESSION_TTL_HOURS")?,
            ),
            max_request_bytes: parse_or(
                get("A2M_MAX_REQUEST_BYTES"),
                1024 * 1024,
                "A2M_MAX_REQUEST_BYTES",
            )?,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.admin_email.is_some() != self.admin_password.is_some() {
            bail!("A2M_ADMIN_EMAIL and A2M_ADMIN_PASSWORD must be set together or not at all");
        }
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            bail!(
                "A2M_BASE_URL must start with http:// or https://, got {:?}",
                self.base_url
            );
        }
        Ok(())
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Log the resolved configuration at startup. Deliberately exhaustive — a config surprise
    /// is far cheaper to diagnose from one line than from a reproduction.
    pub fn log_summary(&self) {
        tracing::info!(
            database_url = %redact_db_url(&self.database_url),
            bind = %self.bind_addr(),
            base_url = %self.base_url,
            default_endpoint = %self.default_endpoint,
            admin_seed = self.admin_email.is_some(),
            run_retention_days = self.run_retention_days,
            allow_loopback_upstream = self.allow_loopback_upstream,
            "configuration"
        );
        if self.allow_loopback_upstream {
            tracing::warn!(
                "A2M_ALLOW_LOOPBACK_UPSTREAM is on — the SSRF guard accepts loopback upstreams"
            );
        }
    }
}

/// Upstream credentials, read by env key name. Held separately from [`Config`] because the
/// values are [`crate::secret::Secret`] and must not be part of a `Debug`-able struct.
pub fn credential_env_keys(get_all: impl Fn() -> Vec<(String, String)>) -> Vec<String> {
    let mut keys: Vec<String> = get_all()
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(CREDENTIAL_PREFIX))
        .collect();
    keys.sort();
    keys
}

/// Replace the password in a Postgres DSN with `***`. Returns the input unchanged when it does
/// not parse as a URL — a malformed DSN is about to fail loudly anyway, and guessing at its
/// structure risks printing the very thing we are hiding.
pub fn redact_db_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((creds, tail)) = rest.split_once('@') else {
        return url.to_string();
    };
    match creds.split_once(':') {
        Some((user, _)) => format!("{scheme}://{user}:***@{tail}"),
        None => format!("{scheme}://{creds}@{tail}"),
    }
}

/// An empty value falls back to the default; a malformed one is an error. A typo must never
/// silently become the default — that is how a production box ends up on a dev setting.
fn parse_or<T>(v: Option<String>, default: T, key: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match v {
        None => Ok(default),
        Some(s) => s
            .parse()
            .map_err(|e| anyhow::anyhow!("{key} must be a number: {e}")),
    }
}

fn parse_bool(v: Option<String>) -> Result<bool> {
    Ok(matches!(
        v.as_deref().map(str::trim),
        Some("1" | "true" | "TRUE" | "yes" | "on")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn database_url_is_required() {
        assert!(Config::from_lookup(lookup(&[])).is_err());
    }

    #[test]
    fn defaults_apply_and_base_url_is_derived() {
        let c = Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://x/y")])).unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.base_url, "http://127.0.0.1:8080");
        assert_eq!(c.default_endpoint, "default");
        assert_eq!(c.run_retention_days, 30);
        assert!(!c.allow_loopback_upstream);
    }

    #[test]
    fn empty_value_falls_back_but_garbage_errors() {
        let ok = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("A2M_PORT", "  "),
        ]));
        assert_eq!(ok.unwrap().port, 8080);
        let bad = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("A2M_PORT", "eighty"),
        ]));
        assert!(bad.unwrap_err().to_string().contains("A2M_PORT"));
    }

    #[test]
    fn base_url_loses_its_trailing_slash() {
        let c = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("A2M_BASE_URL", "https://tools.example.com/"),
        ]))
        .unwrap();
        assert_eq!(c.base_url, "https://tools.example.com");
    }

    #[test]
    fn base_url_must_carry_a_scheme() {
        let e = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("A2M_BASE_URL", "tools.example.com"),
        ]));
        assert!(e.unwrap_err().to_string().contains("A2M_BASE_URL"));
    }

    #[test]
    fn admin_seed_needs_both_halves() {
        let e = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("A2M_ADMIN_EMAIL", "a@b.c"),
        ]));
        assert!(e.is_err());
    }

    #[test]
    fn db_url_password_is_redacted() {
        assert_eq!(
            redact_db_url("postgres://u:hunter2@h:5432/d"),
            "postgres://u:***@h:5432/d"
        );
        assert_eq!(redact_db_url("postgres://h:5432/d"), "postgres://h:5432/d");
        assert_eq!(redact_db_url("not a url"), "not a url");
    }

    #[test]
    fn credential_keys_are_filtered_and_sorted() {
        let keys = credential_env_keys(|| {
            vec![
                ("A2M_CRED_Z".into(), "s".into()),
                ("PATH".into(), "/bin".into()),
                ("A2M_CRED_A".into(), "s".into()),
            ]
        });
        assert_eq!(keys, vec!["A2M_CRED_A", "A2M_CRED_Z"]);
    }
}
