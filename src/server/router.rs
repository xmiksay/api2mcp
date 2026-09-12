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
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::observe;

use super::api;
use super::embed;
use super::login::{self, LoginForm, LoginQuery};
use super::login_oidc::{self, OidcCallbackQuery};
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
        .route("/login/oidc/start", get(get_oidc_start))
        .route("/login/oidc/callback", get(get_oidc_callback))
        .route("/logout", get(get_logout))
        .nest("/api", api::router())
        .merge(oauth::router())
        .route("/static/{*path}", get(embed::static_handler))
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

// `server::login`/`server::login_oidc`'s handlers take plain arguments rather than axum
// extractors bound to `AppState` — `login` predates this module and says so in its own doc, and
// `login_oidc` follows the same shape for consistency. These are the "one-line real handler that
// does the axum-specific extraction and calls straight through" that doc asked for.

async fn get_login(State(state): State<AppState>, Query(query): Query<LoginQuery>) -> Response {
    login::get_login(&state.cfg, query.next.as_deref(), query.error.is_some())
}

async fn post_login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    login::post_login(&state.db, &state.cfg, form).await
}

async fn get_logout(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
    login::get_logout(&state.db, &state.cfg, cookie_header).await
}

/// `GET /login/oidc/start` and `.../callback` are no-ops (redirect straight back to `/login`)
/// when no provider is configured — the router always registers the routes, but there is
/// nothing for them to do without `state.cfg.oidc`, and a 404 here would be a worse signal than
/// "there's nothing to sign in with" for a browser that somehow still reaches this URL.
async fn get_oidc_start(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
) -> Response {
    match &state.cfg.oidc {
        Some(oidc_cfg) => {
            login_oidc::get_oidc_start(&state.cfg, oidc_cfg, query.next.as_deref()).await
        }
        None => Redirect::to("/login").into_response(),
    }
}

async fn get_oidc_callback(
    State(state): State<AppState>,
    Query(query): Query<OidcCallbackQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    match &state.cfg.oidc {
        Some(oidc_cfg) => {
            let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
            login_oidc::get_oidc_callback(&state.db, &state.cfg, oidc_cfg, query, cookie_header)
                .await
        }
        None => Redirect::to("/login").into_response(),
    }
}

// `get_login`/`get_oidc_start`/`get_oidc_callback` all now take `State<AppState>`, which needs a
// real `DatabaseConnection` to construct — not available to a `src/`-local unit test. The
// extraction glue these functions add over `login`/`login_oidc`'s own (already-tested) logic is
// covered end-to-end instead, through the real router, by `tests/auth.rs` and
// `tests/oidc_login.rs`.
