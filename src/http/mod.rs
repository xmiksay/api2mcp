//! The upstream HTTP client, and the physical enforcement points for I2, I3 and I4.
//!
//! This module ships in two chunks. C3 (this one) is the pure, zero-network half: URL
//! templates (I3, `url_template.rs`), origin checking (I2, `origin.rs`), the request binder
//! (`bind.rs`), the SSRF guard's IP-classification table (`ssrf.rs`), and redaction (I4's other
//! half alongside [`crate::secret::Secret`], `redact.rs`). Sending a request, following
//! redirects, and pagination are C4's `http::send` — deliberately absent here so everything in
//! this module is unit-testable with no network access.

mod bind;
mod origin;
mod redact;
mod ssrf;
mod url_template;

pub use bind::{BoundRequest, bind};
pub use origin::assert_allowed;
pub use redact::{redact_headers, redact_message, redact_url};
pub use ssrf::{IpVerdict, SsrfPolicy, check_url, classify};
pub use url_template::{Segment, TemplateError, UrlTemplate};

// I4 spans two modules — `secret` (no `Display`/`Serialize` on `Secret` itself) and this one
// (`redact.rs`, the audit-safe rendering of a request). Re-exporting `CredError` here means a
// caller who wants "every error type below the model boundary" can import them all from
// `http::` without also needing to know Secret's error lives one module over.
pub use crate::secret::CredError;

use serde::Serialize;
use thiserror::Error;

/// Errors from [`bind`]. Model-visible — a bad tool call or a malformed definition can trigger
/// these — so `thiserror` + `Serialize`, never `anyhow`: a `String` context could otherwise
/// smuggle a URL with userinfo, or worse, into a tool response.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum BindError {
    #[error("path template: {0}")]
    Template(#[from] TemplateError),

    #[error("header {name:?} is not in HEADER_PARAM_ALLOWLIST")]
    HeaderNotAllowed { name: String },

    #[error("header {name:?} value contains a CR or LF byte")]
    HeaderInvalidValue { name: String },

    #[error("param {name:?} has a {shape} value, which this param location cannot carry")]
    UnsupportedValueShape { name: String, shape: &'static str },

    #[error("param {name:?} is location Local, which never binds into an HTTP request")]
    LocalParamNotBindable { name: String },

    #[error("body assignment at pointer {pointer:?} failed: {message}")]
    BodyAssign { pointer: String, message: String },

    #[error("service base_url has no host")]
    NoHost,

    #[error("assembled URL failed to parse: {0}")]
    UrlParse(String),

    #[error(
        "assembled request would not stay within the service's origin (I3 containment check failed)"
    )]
    ContainmentViolation,
}

/// Errors from [`assert_allowed`] and [`check_url`] — the origin-allowlist and SSRF checks (I2
/// and the SSRF guard).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum GuardError {
    #[error("scheme {scheme:?} is not http or https")]
    UnsupportedScheme { scheme: String },

    #[error("origin {origin} is not in the service's allowlist")]
    OriginNotAllowed { origin: String },

    #[error("URL has no host")]
    NoHost,

    #[error("host {host:?} is a literal IP address in a denied range ({range})")]
    DeniedLiteralIp { host: String, range: &'static str },
}
