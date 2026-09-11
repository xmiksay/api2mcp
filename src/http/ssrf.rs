//! The SSRF guard's wired-in half: scheme, origin-allowlist, and literal-IP checking on a URL.
//!
//! Why literal-IP checking has to happen here rather than only inside a custom resolver:
//! `hyper-util`'s `HttpConnector` calls `dns::SocketAddrs::try_parse(host, port)` *before* ever
//! consulting a custom `reqwest::dns::Resolve`, so `http://169.254.169.254/` never reaches a
//! resolver-based guard at all. [`check_url`] catches exactly that case, and — per the plan —
//! C4's manual redirect loop must re-run it on every hop, not just the first request.

use std::collections::BTreeSet;
use std::net::IpAddr;

use crate::model::Origin;

use super::GuardError;
use super::origin::assert_allowed;

#[path = "ssrf_deny_table.rs"]
mod deny_table;

pub use deny_table::{IpVerdict, classify};

/// Runtime knobs for the guard that aren't a property of an IP address itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SsrfPolicy {
    /// Lets a literal loopback address through [`check_url`] regardless of [`classify`]'s
    /// verdict. Defaults to `false`; the C4 integration fixture (a real `axum::serve` on
    /// `127.0.0.1:0`) sets it via `A2M_ALLOW_LOOPBACK_UPSTREAM=1` — production traffic never
    /// needs this.
    pub allow_loopback: bool,
}

/// Scheme, origin-allowlist, and literal-IP checks on `url`. Does **not** resolve a hostname —
/// that's the DNS backend's job (C4); this only catches a hostname that is *already* an IP
/// literal, the exact case hyper's connector special-cases and a resolver never sees.
pub fn check_url(
    url: &url::Url,
    allowlist: &BTreeSet<Origin>,
    policy: &SsrfPolicy,
) -> Result<(), GuardError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GuardError::UnsupportedScheme {
            scheme: url.scheme().to_owned(),
        });
    }

    let origin = Origin::of(url).map_err(|_| GuardError::NoHost)?;
    assert_allowed(&origin, allowlist)?;

    let host = url.host_str().ok_or(GuardError::NoHost)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        check_ip(ip, policy)?;
    }
    Ok(())
}

fn check_ip(ip: IpAddr, policy: &SsrfPolicy) -> Result<(), GuardError> {
    if policy.allow_loopback && ip.is_loopback() {
        return Ok(());
    }
    match classify(ip) {
        IpVerdict::Allowed => Ok(()),
        IpVerdict::Denied(range) => Err(GuardError::DeniedLiteralIp {
            host: ip.to_string(),
            range,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(s: &str) -> Origin {
        s.parse().expect("valid origin")
    }

    fn allowlist(origins: &[&str]) -> BTreeSet<Origin> {
        origins.iter().map(|s| origin(s)).collect()
    }

    #[test]
    fn rejects_non_http_scheme() {
        let url = url::Url::parse("ftp://example.com/").expect("valid url");
        let err = check_url(&url, &BTreeSet::new(), &SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, GuardError::UnsupportedScheme { .. }));
    }

    #[test]
    fn rejects_origin_not_in_allowlist() {
        let url = url::Url::parse("https://evil.example.com/").expect("valid url");
        let list = allowlist(&["https://api.example.com"]);
        let err = check_url(&url, &list, &SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, GuardError::OriginNotAllowed { .. }));
    }

    #[test]
    fn rejects_literal_link_local_ip() {
        let url = url::Url::parse("http://169.254.169.254/latest/meta-data").expect("valid url");
        let list = allowlist(&["http://169.254.169.254"]);
        let err = check_url(&url, &list, &SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, GuardError::DeniedLiteralIp { .. }));
    }

    #[test]
    fn allows_public_literal_ip_in_allowlist() {
        let url = url::Url::parse("http://93.184.216.34/").expect("valid url");
        let list = allowlist(&["http://93.184.216.34"]);
        assert!(check_url(&url, &list, &SsrfPolicy::default()).is_ok());
    }

    #[test]
    fn loopback_denied_by_default() {
        let url = url::Url::parse("http://127.0.0.1:9999/").expect("valid url");
        let list = allowlist(&["http://127.0.0.1:9999"]);
        assert!(check_url(&url, &list, &SsrfPolicy::default()).is_err());
    }

    #[test]
    fn loopback_allowed_when_policy_opts_in() {
        let url = url::Url::parse("http://127.0.0.1:9999/").expect("valid url");
        let list = allowlist(&["http://127.0.0.1:9999"]);
        let policy = SsrfPolicy {
            allow_loopback: true,
        };
        assert!(check_url(&url, &list, &policy).is_ok());
    }

    #[test]
    fn hostname_is_not_ip_checked() {
        // A DNS name isn't a literal IP, so check_url passes it through untouched — resolving
        // and re-checking the resolved address is the DNS backend's job (C4), not this
        // pre-flight check's.
        let url = url::Url::parse("https://api.example.com/").expect("valid url");
        let list = allowlist(&["https://api.example.com"]);
        assert!(check_url(&url, &list, &SsrfPolicy::default()).is_ok());
    }
}
