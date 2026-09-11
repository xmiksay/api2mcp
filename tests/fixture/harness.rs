//! Shared `model`/`http` value builders for tests driving [`super::Fixture`] through
//! `http::send`/`http::paginate` — split out from `mod.rs` (which owns the server itself) purely
//! to keep both files under the workspace's 400-line cap.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use api2mcp::http::{self, SendParams, SsrfPolicy, StaticDns, UpstreamPool};
use api2mcp::model::{Origin, Service, Slug};

use super::Fixture;

pub fn slug(s: &str) -> Slug {
    Slug::from_str(s).expect("valid slug")
}

/// A `Service` whose `base_url` is the fixture's own literal-IP address. The SSRF guard's
/// `check_url` rejects loopback by default (see `SsrfPolicy`), so every test using this opts in
/// via `allow_loopback: true` — exactly the escape hatch `Config::allow_loopback_upstream` and
/// design correction #9 describe.
pub fn service_for(fixture: &Fixture) -> Service {
    let origin = Origin::of(&fixture.base_url()).expect("fixture base url has a valid origin");
    Service {
        slug: slug("demo"),
        base_url: fixture.base_url(),
        origin_allowlist: BTreeSet::from([origin]),
        default_headers: BTreeMap::new(),
        timeout_ms: 2_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 64 * 1024 * 1024,
    }
}

/// Since the fixture is addressed by a literal IP, hyper's connector never consults a resolver
/// for it at all (see `http::resolver`'s module docs) — so an empty `StaticDns` is a safe
/// placeholder pool for every test that only ever talks to `127.0.0.1` directly.
pub fn loopback_pool() -> UpstreamPool {
    UpstreamPool::new(
        Arc::new(StaticDns::new()),
        SsrfPolicy {
            allow_loopback: true,
        },
    )
}

pub fn bound_get(url: url::Url) -> http::BoundRequest {
    http::BoundRequest {
        method: ::http::Method::GET,
        url,
        headers: BTreeMap::new(),
        body: None,
    }
}

pub fn send_params<'a>(service: &'a Service, policy: &'a SsrfPolicy) -> SendParams<'a> {
    SendParams {
        allowlist: &service.origin_allowlist,
        policy,
        auth: None,
        max_response_bytes: service.max_response_bytes,
        deadline: Duration::from_secs(5),
        max_redirects: 5,
    }
}
