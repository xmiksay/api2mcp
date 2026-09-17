//! `AppState`: the handful of heavyweight singletons every request handler needs. Deliberately
//! small — [`AppState::stores`] builds a fresh [`Stores`] façade on demand (chess-base's
//! on-demand-façade pattern), so adding a new store aggregate never means adding a new field
//! here.
//!
//! Two fields the skeleton plan sketched for this struct don't appear, both for the same reason:
//! nothing in the crate as built actually needs them held centrally.
//! - `limits` (a `runtime::fanout::ConcurrencyLimits`): built fresh per tool call from *that
//!   call's* `EndpointPlan` (`ConcurrencyLimits::build`, inside `Executor::run_tool`) — there is
//!   no cross-request concurrency state to hold, only a per-run one already owned elsewhere.
//! - `secrets` (a `CredentialResolver`): never implemented in `secret/`. Every credential read
//!   goes straight through `secret::Secret::load(env_key)` at its one call site
//!   (`http::auth::apply`), so there is nothing for a resolver type to cache.
//!
//! Both are noted here rather than silently dropped so a later chunk that actually needs
//! request-scoped concurrency limiting or credential caching knows this is where the plan
//! expected it and why it isn't.

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::config::Config;
use crate::http::{SsrfPolicy, UpstreamPool};
use crate::resolve::PlanCache;
use crate::store::Stores;

use super::identity::AuthContext;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub cfg: Arc<Config>,
    /// One `reqwest::Client` per service, guarded DNS resolver, SSRF policy baked in at
    /// construction — see `http::UpstreamPool`.
    pub upstream: Arc<UpstreamPool>,
    /// Compiled `EndpointPlan`s, generation-guarded against `meta.definitions_generation`.
    pub plans: Arc<PlanCache>,
}

impl AppState {
    pub fn new(
        db: DatabaseConnection,
        cfg: Arc<Config>,
        upstream: Arc<UpstreamPool>,
        plans: Arc<PlanCache>,
    ) -> Self {
        Self {
            db,
            cfg,
            upstream,
            plans,
        }
    }

    /// One façade per aggregate, built fresh from the shared connection — see `store::Stores`'s
    /// own module doc for why this is a method here, not a field per aggregate.
    pub fn stores(&self) -> Stores {
        Stores::new(self.db.clone())
    }

    /// The SSRF policy every `runtime::Executor` this process builds must share. Derived from
    /// `cfg` on every call rather than cached — it is one `bool` wide, so caching it would only
    /// add a field for no measurable benefit.
    pub fn ssrf_policy(&self) -> SsrfPolicy {
        ssrf_policy_for(&self.cfg)
    }
}

fn ssrf_policy_for(cfg: &Config) -> SsrfPolicy {
    SsrfPolicy {
        allow_loopback: cfg.allow_loopback_upstream,
    }
}

impl AuthContext for AppState {
    fn auth_db(&self) -> &DatabaseConnection {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cfg(allow_loopback: bool) -> Config {
        Config {
            database_url: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            base_url: "http://h:8080".into(),
            seed_email: None,
            seed_password: None,
            run_retention_days: 30,
            allow_loopback_upstream: allow_loopback,
            session_ttl: Duration::from_secs(3600),
            max_request_bytes: 1024 * 1024,
            oidc: None,
        }
    }

    #[test]
    fn ssrf_policy_reflects_the_config_flag() {
        assert!(ssrf_policy_for(&cfg(true)).allow_loopback);
        assert!(!ssrf_policy_for(&cfg(false)).allow_loopback);
    }
}
