//! A single named HTTP call: method, templated path, fixed query/body, and the params that
//! parameterise it. `path_template`/`body_template` stay raw strings/JSON here — compiling the
//! path template into segments (I3) is `http::url_template`'s job, at resolve time, not this
//! module's.

use std::collections::BTreeMap;

use serde_json::Value;

use super::param::Param;
use super::projection::Projection;
use super::slug::Slug;

/// Read calls are always safe to retry-free-run; write calls need an endpoint's `write_ceiling`
/// to explicitly permit them. Declared in variant order `Read < Write` so a ceiling comparison
/// (`api_call.access <= endpoint.write_ceiling`) reads naturally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Write,
}

/// How to advance to the next page of a paginated response. This is the *mechanism*; the
/// *ceiling* on how many pages to fetch is `Budgets::max_pages` (I6) — a different question,
/// enforced by the runtime, not declared here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pagination {
    None,
    /// A cursor value read from the previous response is written into the next request's query
    /// string.
    Cursor {
        /// JSON pointer into the response body where the next cursor lives.
        next_cursor_path: jsonptr::PointerBuf,
        /// The query param that carries the cursor value on the next call.
        query_param: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiCall {
    pub slug: Slug,
    pub service_slug: Slug,
    pub auth_provider_slug: Option<Slug>,
    pub method: http::Method,
    /// Raw template, e.g. `/users/{id}`; compiled by `http::url_template` at resolve time.
    pub path_template: String,
    pub query_fixed: BTreeMap<String, String>,
    pub body_template: Option<Value>,
    pub access: Access,
    pub idempotent: bool,
    pub projection: Option<Projection>,
    pub pagination: Pagination,
    /// Overrides `Service::timeout_ms`/`max_response_bytes` when set.
    pub timeout_ms: Option<u32>,
    pub max_response_bytes: Option<u64>,
    pub params: Vec<Param>,
    /// Human-authored documentation of what this call returns and when to reach for it —
    /// the MCP tool `description` a model reads to decide whether and how to call the tool.
    /// `server::mcp::registry::describe` falls back to a synthesized method/path string only
    /// when this is `None`.
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_ordering_puts_read_below_write() {
        assert!(Access::Read < Access::Write);
    }
}
