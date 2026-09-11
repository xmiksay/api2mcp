//! The SSRF hook, wired into `reqwest` as a custom [`Resolve`] implementation rather than
//! `ClientBuilder::resolve()` overrides. Three verified reasons this has to be a custom
//! resolver:
//!
//! - `.resolve()` overrides are layered *on top of* a custom resolver
//!   (`reqwest::async_impl::client`'s `dns_overrides` path), so a guard installed only there
//!   would be silently bypassable by anything using the override map.
//! - `.resolve()` pins addresses per **client**, so pinning per request would mean a client per
//!   request — destroying connection pooling.
//! - A pre-flight resolve followed by a separate connect *creates* the DNS-rebinding window
//!   instead of closing it. Resolving inside the connector's own resolver call (this module) makes
//!   resolve-and-connect atomic, and leaves the URL — and therefore the `Host` header, SNI and
//!   certificate validation — completely untouched.
//!
//! [`GuardedResolver`] itself never touches the network — it delegates the actual lookup to a
//! [`DnsBackend`], so tests can supply [`StaticDns`] or [`RebindDns`] with no DNS traffic at all.
//! Production wires up [`HickoryDns`].

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use hickory_resolver::TokioResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use thiserror::Error;
use tokio::sync::OnceCell;

use super::ssrf::{IpVerdict, SsrfPolicy, classify};

/// Turns a hostname into candidate addresses. The abstraction that lets every SSRF-adjacent test
/// run with no real DNS traffic.
#[async_trait]
pub trait DnsBackend: Send + Sync + 'static {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, DnsError>;
}

/// Errors from a [`DnsBackend`] lookup or from the guard filtering every candidate out. Never
/// carries anything beyond a hostname and a message — a DNS error string is not credential
/// material, so this doesn't need `http::redact`'s treatment, but it also has no reason to carry
/// more than that.
#[derive(Debug, Error)]
pub enum DnsError {
    #[error("resolving {host:?}: {message}")]
    Backend { host: String, message: String },
    #[error("no address for {host:?} survived the SSRF filter")]
    AllAddressesDenied { host: String },
}

/// Production DNS backend: `hickory-resolver` over the system configuration. Built lazily behind
/// a [`OnceCell`] because constructing a `TokioResolver` must happen inside a Tokio context, and
/// this type may itself be constructed before one exists (e.g. at process startup).
#[derive(Default)]
pub struct HickoryDns {
    resolver: OnceCell<TokioResolver>,
}

impl HickoryDns {
    pub fn new() -> Self {
        Self::default()
    }

    async fn resolver(&self) -> Result<&TokioResolver, DnsError> {
        self.resolver
            .get_or_try_init(|| async {
                let builder = TokioResolver::builder_tokio().map_err(|e| DnsError::Backend {
                    host: String::new(),
                    message: format!("building resolver config: {e}"),
                })?;
                builder.build().map_err(|e| DnsError::Backend {
                    host: String::new(),
                    message: format!("building resolver: {e}"),
                })
            })
            .await
    }
}

#[async_trait]
impl DnsBackend for HickoryDns {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, DnsError> {
        let resolver = self.resolver().await?;
        let lookup = resolver
            .lookup_ip(host)
            .await
            .map_err(|e| DnsError::Backend {
                host: host.to_owned(),
                message: e.to_string(),
            })?;
        Ok(lookup.into_iter().collect())
    }
}

/// Test backend: a fixed hostname-to-addresses table. A lookup for a hostname outside the table
/// errors rather than falling through to anything real — a typo'd hostname in a test must fail
/// loudly, never quietly hit the network.
#[derive(Debug, Clone, Default)]
pub struct StaticDns(std::collections::BTreeMap<String, Vec<IpAddr>>);

impl StaticDns {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, host: &str, addrs: Vec<IpAddr>) -> Self {
        self.0.insert(host.to_owned(), addrs);
        self
    }
}

#[async_trait]
impl DnsBackend for StaticDns {
    async fn lookup(&self, host: &str) -> Result<Vec<IpAddr>, DnsError> {
        self.0.get(host).cloned().ok_or_else(|| DnsError::Backend {
            host: host.to_owned(),
            message: "no static entry for this host".to_owned(),
        })
    }
}

/// Test backend that returns a different address list on each successive lookup of any host —
/// models a DNS-rebinding attacker. It exists to demonstrate, not to defeat: because
/// [`GuardedResolver`] resolves atomically inside the connector's own call, every individual
/// connection attempt still only ever sees and dials the addresses *that one* lookup returned,
/// and those still go through the same [`classify`] filter — the rebinding window a pre-flight
/// resolve would open never exists here.
pub struct RebindDns {
    sequence: Vec<Vec<IpAddr>>,
    call: AtomicUsize,
}

impl RebindDns {
    pub fn new(sequence: Vec<Vec<IpAddr>>) -> Self {
        assert!(!sequence.is_empty(), "RebindDns needs at least one entry");
        Self {
            sequence,
            call: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DnsBackend for RebindDns {
    async fn lookup(&self, _host: &str) -> Result<Vec<IpAddr>, DnsError> {
        let i = self.call.fetch_add(1, Ordering::SeqCst);
        Ok(self.sequence[i.min(self.sequence.len() - 1)].clone())
    }
}

/// A `reqwest::dns::Resolve` that resolves through a [`DnsBackend`], filters every returned
/// address through [`classify`], sorts and dedups the survivors (determinism, I7), and fails the
/// connection outright when nothing survives.
pub struct GuardedResolver {
    backend: Arc<dyn DnsBackend>,
    policy: SsrfPolicy,
}

impl GuardedResolver {
    pub fn new(backend: Arc<dyn DnsBackend>, policy: SsrfPolicy) -> Self {
        Self { backend, policy }
    }
}

impl fmt::Debug for GuardedResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardedResolver").finish_non_exhaustive()
    }
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let backend = Arc::clone(&self.backend);
        let policy = self.policy;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let candidates = backend.lookup(&host).await?;
            // A `BTreeSet` sorts and dedups in one step — the property I7 needs from this
            // resolver regardless of what order the backend produced.
            let mut survivors: BTreeSet<IpAddr> = BTreeSet::new();
            for ip in candidates {
                let allowed = (policy.allow_loopback && ip.is_loopback())
                    || matches!(classify(ip), IpVerdict::Allowed);
                if allowed {
                    survivors.insert(ip);
                }
            }
            if survivors.is_empty() {
                return Err(Box::new(DnsError::AllAddressesDenied { host })
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            // Port 0: reqwest fills in the real port (explicit in the URL, or the scheme
            // default) itself once this resolver returns — see `Resolve::resolve`'s docs.
            let addrs: Addrs = Box::new(survivors.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::from([a, b, c, d])
    }

    async fn resolve_names(resolver: &GuardedResolver, host: &str) -> Result<Vec<IpAddr>, String> {
        let name: Name = host.parse().expect("valid dns name");
        let addrs = resolver.resolve(name).await.map_err(|e| e.to_string())?;
        Ok(addrs.map(|s| s.ip()).collect())
    }

    #[tokio::test]
    async fn filters_out_denied_addresses_and_keeps_allowed_ones() {
        let backend = StaticDns::new().with(
            "mixed.test",
            vec![v4(169, 254, 169, 254), v4(93, 184, 216, 34)],
        );
        let resolver = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        let addrs = resolve_names(&resolver, "mixed.test").await.expect("ok");
        assert_eq!(addrs, vec![v4(93, 184, 216, 34)]);
    }

    #[tokio::test]
    async fn fails_the_connect_when_every_address_is_denied() {
        let backend = StaticDns::new().with("evil.test", vec![v4(169, 254, 169, 254)]);
        let resolver = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        let err = resolve_names(&resolver, "evil.test").await.unwrap_err();
        assert!(err.contains("survived"));
    }

    #[tokio::test]
    async fn sorts_and_dedups_survivors() {
        let backend = StaticDns::new().with(
            "dup.test",
            vec![v4(93, 184, 216, 34), v4(1, 1, 1, 1), v4(93, 184, 216, 34)],
        );
        let resolver = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        let addrs = resolve_names(&resolver, "dup.test").await.expect("ok");
        assert_eq!(addrs, vec![v4(1, 1, 1, 1), v4(93, 184, 216, 34)]);
    }

    #[tokio::test]
    async fn loopback_survives_only_when_the_policy_opts_in() {
        let backend = StaticDns::new().with("loop.test", vec![v4(127, 0, 0, 1)]);
        let denied = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        assert!(resolve_names(&denied, "loop.test").await.is_err());

        let backend = StaticDns::new().with("loop.test", vec![v4(127, 0, 0, 1)]);
        let allowed = GuardedResolver::new(
            Arc::new(backend),
            SsrfPolicy {
                allow_loopback: true,
            },
        );
        assert_eq!(
            resolve_names(&allowed, "loop.test").await.expect("ok"),
            vec![v4(127, 0, 0, 1)]
        );
    }

    #[tokio::test]
    async fn rebind_dns_returns_a_fresh_but_still_guarded_list_each_call() {
        let backend = RebindDns::new(vec![
            vec![v4(93, 184, 216, 34)],
            vec![v4(169, 254, 169, 254)],
        ]);
        let resolver = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        assert_eq!(
            resolve_names(&resolver, "rebind.test").await.expect("ok"),
            vec![v4(93, 184, 216, 34)]
        );
        // Second lookup returns the metadata address — still filtered out by the same guard,
        // because resolution happens fresh and atomically on every connect, never cached ahead
        // of time.
        assert!(resolve_names(&resolver, "rebind.test").await.is_err());
    }

    #[tokio::test]
    async fn backend_error_surfaces_as_a_resolve_error() {
        let backend = StaticDns::new();
        let resolver = GuardedResolver::new(Arc::new(backend), SsrfPolicy::default());
        let err = resolve_names(&resolver, "missing.test").await.unwrap_err();
        assert!(err.contains("no static entry"));
    }
}
