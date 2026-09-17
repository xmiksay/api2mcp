//! OAuth 2.1 authorization server, public-client / PKCE-only: RFC 9728/8414
//! discovery, RFC 7591 dynamic client registration (so Claude Code self-onboards with no
//! pre-provisioned client), the authorization-code grant with mandatory PKCE S256, a
//! server-rendered consent screen, and the refresh-token grant (rotation, family reuse
//! detection, and an absolute family TTL — see `refresh`'s module doc).
//!
//! Every handler here returns the OAuth error-response shape (`error`/`error_description`,
//! [`OAuthError`]) rather than [`crate::server::error::ApiError`] — an OAuth client parses
//! this shape, not ours. Every token/code/verifier is sha256-hashed at rest by
//! [`crate::store::oauth`]/`oauth_tokens` and is never logged or formatted anywhere in this
//! module.
//!
//! [`authorize`] and [`consent`] are server-rendered, not SPA views, for the same two reasons
//! `server::login` gives: `web/dist` is a `build.rs` placeholder on a fresh clone and in CI,
//! and the whole point of a bearer credential is that it never becomes reachable from
//! JavaScript. [`server::auth::authenticate_mcp`](crate::server::auth::authenticate_mcp)'s
//! OAuth access-token branch is what makes a token minted here actually usable on `/mcp`.

mod authorize;
mod consent;
mod discovery;
mod refresh;
mod register;
mod shared;
mod token;

use axum::Router;
use axum::routing::{get, post};

use super::state::AppState;

/// Mounts the whole authorization-server surface: discovery, DCR, the authorize/consent
/// pair, and the token endpoint. `Router<AppState>` so the caller can `.merge()` it into the
/// rest of the app before the one `.with_state()` call — the same shape as
/// [`crate::server::mcp::router`].
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(discovery::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(discovery::authorization_server_metadata),
        )
        .route("/oauth/register", post(register::register_client))
        .route("/oauth/authorize", get(authorize::authorize))
        .route(
            "/oauth/consent",
            get(consent::get_consent).post(consent::post_consent),
        )
        .route("/oauth/token", post(token::token))
}
