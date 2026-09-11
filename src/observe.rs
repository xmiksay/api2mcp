//! Tracing setup and the request-logging middleware.
//!
//! The span field list is an allowlist, not a filter. Nothing here ever records a header:
//! `Authorization` is the whole of I4 on the inbound side, and a span that records "all
//! headers minus a deny list" is one header name away from leaking a credential.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use tracing::Span;
use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str = "api2mcp=info,tower_http=info,sea_orm=warn,sqlx=warn,info";

pub fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Span for an inbound request. The query string is deliberately dropped — a query can carry
/// caller-supplied values, and this line goes to a log that outlives the request.
pub fn make_span(req: &Request) -> Span {
    tracing::info_span!(
        "http",
        method = %req.method(),
        path = %req.uri().path(),
        status = tracing::field::Empty,
        ms = tracing::field::Empty,
    )
}

pub async fn log_slow_request(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    let ms = started.elapsed().as_millis();
    let span = Span::current();
    span.record("status", resp.status().as_u16());
    span.record("ms", ms as u64);
    if ms >= SLOW_MS {
        tracing::warn!(%method, %path, ms, status = resp.status().as_u16(), "slow request");
    }
    resp
}

const SLOW_MS: u128 = 1_000;
