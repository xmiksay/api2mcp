//! `UpstreamPool`: one `reqwest::Client` per service, built lazily and cached, each wired to
//! [`super::resolver::GuardedResolver`] so I2's origin check and the SSRF guard are the *only*
//! way any of these clients ever open a connection.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::model::{Service, Slug};

use super::resolver::{DnsBackend, GuardedResolver};
use super::ssrf::SsrfPolicy;

#[derive(Debug, Clone, Error)]
#[error("building the upstream HTTP client for service: {0}")]
pub struct ClientBuildError(String);

/// One `reqwest::Client` per service slug, built on first use and cached for the pool's
/// lifetime — a fresh client per call would rebuild the connection pool (and the TLS session
/// cache) on every request; a single shared client across all services would let one hostile or
/// slow service's connections starve every other service's `max_concurrency`.
pub struct UpstreamPool {
    dns: Arc<dyn DnsBackend>,
    policy: SsrfPolicy,
    clients: RwLock<BTreeMap<Slug, Arc<Client>>>,
}

impl UpstreamPool {
    pub fn new(dns: Arc<dyn DnsBackend>, policy: SsrfPolicy) -> Self {
        Self {
            dns,
            policy,
            clients: RwLock::new(BTreeMap::new()),
        }
    }

    /// Returns the cached client for `service`, building and caching one on first use.
    pub async fn client_for(&self, service: &Service) -> Result<Arc<Client>, ClientBuildError> {
        if let Some(existing) = self.clients.read().await.get(&service.slug) {
            return Ok(Arc::clone(existing));
        }
        let mut clients = self.clients.write().await;
        // Another task may have built it while we waited for the write lock.
        if let Some(existing) = clients.get(&service.slug) {
            return Ok(Arc::clone(existing));
        }
        let client = Arc::new(build_client(service, Arc::clone(&self.dns), self.policy)?);
        clients.insert(service.slug.clone(), Arc::clone(&client));
        Ok(client)
    }
}

fn build_client(
    service: &Service,
    dns: Arc<dyn DnsBackend>,
    policy: SsrfPolicy,
) -> Result<Client, ClientBuildError> {
    let resolver = Arc::new(GuardedResolver::new(dns, policy));
    let timeout = Duration::from_millis(u64::from(service.timeout_ms));

    Client::builder()
        .dns_resolver(resolver)
        // The default follows up to 10 redirects, which would walk a 302 straight past I2's
        // origin allowlist before `send`'s manual loop ever gets a say. `send` re-checks every
        // hop itself, so redirects must never be followed here.
        .redirect(reqwest::redirect::Policy::none())
        // reqwest reads HTTP_PROXY/HTTPS_PROXY/ALL_PROXY from the environment by default; with a
        // proxy configured, the *proxy* resolves the hostname and connects on our behalf,
        // bypassing GuardedResolver entirely. This is the one line that closes that hole.
        .no_proxy()
        // A Referer header would leak the current upstream URL (query string included) to
        // whatever the next hop turns out to be — a hop `send`'s own allowlist re-check has no
        // say over once the header is already on the wire.
        .referer(false)
        .default_headers(default_header_map(service))
        .connect_timeout(timeout)
        // Bounds how long a single read can stall once a connection is open — the guard against
        // a slowloris upstream that opens the connection and then trickles bytes forever. This is
        // independent of the per-call wall-clock deadline `send` applies via `.timeout()`.
        .read_timeout(timeout)
        .build()
        .map_err(|e| ClientBuildError(e.to_string()))
}

fn default_header_map(service: &Service) -> ::http::HeaderMap {
    let mut headers = ::http::HeaderMap::new();
    for (name, value) in &service.default_headers {
        let (Ok(name), Ok(value)) = (
            ::http::HeaderName::try_from(name.as_str()),
            ::http::HeaderValue::from_str(value),
        ) else {
            // A malformed default header is a definer-time data problem that publish-time
            // validation (resolve/, not yet built) should have caught; skipping it here rather
            // than panicking keeps client construction infallible on bad data.
            continue;
        };
        headers.insert(name, value);
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::resolver::StaticDns;
    use std::collections::{BTreeMap as StdBTreeMap, BTreeSet};
    use std::str::FromStr;

    fn service(slug: &str) -> Service {
        Service {
            owner_id: uuid::Uuid::nil(),
            slug: Slug::from_str(slug).expect("valid slug"),
            base_url: url::Url::parse("https://api.example.com").expect("valid url"),
            origin_allowlist: BTreeSet::new(),
            default_headers: StdBTreeMap::from([("X-Test".to_owned(), "1".to_owned())]),
            timeout_ms: 1000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn pool() -> UpstreamPool {
        UpstreamPool::new(Arc::new(StaticDns::new()), SsrfPolicy::default())
    }

    #[tokio::test]
    async fn builds_and_caches_one_client_per_service_slug() {
        let pool = pool();
        let a1 = pool.client_for(&service("svc-a")).await.expect("built");
        let a2 = pool.client_for(&service("svc-a")).await.expect("built");
        assert!(Arc::ptr_eq(&a1, &a2), "same slug must reuse the client");

        let b = pool.client_for(&service("svc-b")).await.expect("built");
        assert!(
            !Arc::ptr_eq(&a1, &b),
            "different slugs get different clients"
        );
    }
}
