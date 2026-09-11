//! Router composition and the inbound layer stack. `build_router` is `pub` specifically so
//! `tests/mcp.rs` can drive the whole app through [`tower::ServiceExt::oneshot`] without binding
//! a real port.
//!
//! Layer order (outermost first, i.e. the *last* `.layer()` call below):
//! `Trace` → `CatchPanic` → `Timeout` → `DefaultBodyLimit` → security headers → slow-request log
//! → routes. `TraceLayer` sits outermost so one span covers a panic or a timeout, not just a
//! normal return; `CatchPanicLayer` sits outside `TimeoutLayer` so a panic becomes a `500`
//! instead of a request that hangs until the timeout fires anyway; `DefaultBodyLimit` sits
//! inside the timeout but outside every handler, because `/mcp` reads raw `Bytes` — an unbounded
//! body would otherwise buffer in full before JSON-RPC framing ever gets a chance to reject it.
//!
//! **No `CorsLayer`.** MCP clients are not browsers, and the embedded SPA is served same-origin
//! — a permissive CORS layer here would be a regression, not a convenience.
//!
//! `api::router` (the read-write admin JSON API) is nested at `/api`; `oauth::router` (the
//! authorization server and its two `.well-known` discovery documents) is merged at the root,
//! because RFC 9728 and RFC 8414 both fix those paths.

use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::observe;

use super::api;
use super::embed;
use super::login::{self, LoginForm, LoginQuery};
use super::mcp;
use super::oauth;
use super::state::AppState;

const CSP: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'";

/// Builds the whole app. `pub` so integration tests can `oneshot` it directly.
pub fn build_router(state: AppState) -> Router {
    let max_body = state.cfg.max_request_bytes;

    Router::new()
        .merge(mcp::router())
        .route("/login", get(get_login).post(post_login))
        .route("/logout", get(get_logout))
        .nest("/api", api::router())
        .merge(oauth::router())
        .fallback(embed::spa_handler)
        .with_state(state)
        .layer(axum::middleware::from_fn(observe::log_slow_request))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(DefaultBodyLimit::max(max_body))
        .layer(middleware::from_fn(request_timeout))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http().make_span_with(observe::make_span))
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Bounds total request handling time. Written as ordinary middleware (`tokio::time::timeout`
/// around `next.run`) rather than `tower_http::timeout::TimeoutLayer` + `HandleErrorLayer`: the
/// latter needs its error type resolved through a generic extractor-tuple parameter that fights
/// type inference for no behavioural difference here, and axum requires the whole layer stack's
/// `Error` to end up `Infallible` regardless — this version already is one, directly.
async fn request_timeout(req: Request, next: Next) -> Response {
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(req)).await {
        Ok(response) => response,
        Err(_) => (StatusCode::REQUEST_TIMEOUT, "request timed out").into_response(),
    }
}

// `server::login`'s handlers take plain arguments rather than axum extractors bound to
// `AppState` — it predates this module and says so in its own doc. These three are the "one-line
// real handler that does the axum-specific extraction and calls straight through" it asked for.

async fn get_login(Query(query): Query<LoginQuery>) -> Response {
    login::get_login(query.next.as_deref())
}

async fn post_login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    login::post_login(&state.db, &state.cfg, form).await
}

async fn get_logout(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
    login::get_logout(&state.db, &state.cfg, cookie_header).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a redirect loop: `get_logout`'s inner `login::get_logout` always redirects to
    /// `/login` regardless of whether a session cookie was present — pinning the extraction
    /// glue here rather than re-testing `login`'s own already-tested behaviour.
    #[tokio::test]
    async fn get_login_extraction_glue_calls_through_without_panicking() {
        let query = Query(LoginQuery { next: None });
        let response = get_login(query).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
