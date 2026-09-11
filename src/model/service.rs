//! A `Service` is one upstream API: a base URL, the set of origins any of its api_calls may
//! reach, and connection-level defaults. `resolve/origins.rs` (I2) checks every selected
//! api_call's computed origin against `origin_allowlist` before a plan can exist.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::origin::Origin;
use super::slug::Slug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub slug: Slug,
    pub base_url: url::Url,
    /// Origins any of this service's api_calls may resolve to. Design correction #7: publish-time
    /// validation (not this type) must ensure `base_url`'s own origin is a member — an allowlist
    /// that excludes the thing it's supposedly bounding is trivially inconsistent.
    pub origin_allowlist: BTreeSet<Origin>,
    pub default_headers: BTreeMap<String, String>,
    pub timeout_ms: u32,
    pub max_concurrency: u32,
    pub rate_limit_per_min: Option<u32>,
    pub max_response_bytes: u64,
}
