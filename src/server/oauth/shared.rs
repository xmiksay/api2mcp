//! Shared helpers for the `oauth` module: the OAuth error-response shape (never
//! `server::error::ApiError` — a client parses `error`/`error_description`, not our shape),
//! PKCE S256 verification, percent-encoding for redirect query values, and resolving the
//! logged-in caller from the session cookie `server::login` already sets.

use axum::Json;
use axum::http::StatusCode;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::server::auth;
use crate::server::identity::Caller;
use crate::server::state::AppState;

/// An OAuth 2.1 error response. Every handler in this module returns this, never
/// `server::error::ApiError` — the two shapes must never mix on this surface.
#[derive(Debug)]
pub struct OAuthError {
    pub status: StatusCode,
    pub error: &'static str,
    pub description: String,
}

impl OAuthError {
    pub fn bad(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error,
            description: description.into(),
        }
    }

    pub fn server_error(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "server_error",
            description: description.into(),
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.error, "error_description": self.description })),
        )
            .into_response()
    }
}

/// A `StoreError` reaching this module is never something the client caused (a schema-level
/// malformed row, or a DB outage) — always `server_error`, and never forwarding
/// `StoreError`'s own message verbatim, keeping that mapping decision in one place.
impl From<crate::store::StoreError> for OAuthError {
    fn from(_: crate::store::StoreError) -> Self {
        OAuthError::server_error("internal error")
    }
}

/// PKCE S256 check: `base64url(SHA-256(verifier)) == code_challenge` (no padding). There is
/// no `plain` counterpart in this module — `plain` is refused simply by this being the only
/// verification function that exists.
pub fn verify_pkce_s256(verifier: &str, challenge: &str) -> bool {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == challenge
}

/// Percent-encodes a value for safe inclusion in a redirect's query string (a `code`, a
/// `state`, a `next` path). Over-encodes a few unreserved characters (`-_.~`) rather than
/// hand-tuning the allowed set — harmless for a query value, and one fewer thing to get
/// subtly wrong.
pub fn percent_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// Builds a token endpoint success response (RFC 6749 §5.1). `scope` is omitted when the
/// caller has none worth echoing back (the refresh grant doesn't re-resolve it — see
/// `refresh::grant`'s doc for why that's fine per spec).
pub fn token_response(
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in: i64,
    scope: Option<&str>,
) -> Response {
    let mut body = json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": expires_in,
    });
    if let Some(rt) = refresh_token {
        body["refresh_token"] = json!(rt);
    }
    if let Some(s) = scope {
        body["scope"] = json!(s);
    }
    (StatusCode::OK, Json(body)).into_response()
}

/// Resolves the browser session cookie (if any) to its [`Caller`] — the same session
/// `server::login::post_login` creates and `server::identity::Caller`'s `FromRequestParts`
/// impl reads, duplicated here (rather than using that extractor) because an OAuth handler
/// needs to *redirect to login* on a miss, not axum's bare `401` rejection.
pub async fn current_caller(state: &AppState, headers: &HeaderMap) -> Option<Caller> {
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())?;
    let token = auth::cookie_value(cookie_header, auth::SESSION_COOKIE_NAME)?;
    auth::resolve_session(&state.db, token).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifies_a_known_s256_pair() {
        // RFC 7636 appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_pkce_s256(verifier, challenge));
        assert!(!verify_pkce_s256("wrong", challenge));
    }

    #[test]
    fn percent_encode_escapes_query_metacharacters() {
        let encoded = percent_encode("a b&c=d");
        assert_eq!(encoded, "a%20b%26c%3Dd");
    }

    #[test]
    fn oauth_error_response_carries_the_error_shape() {
        let response = OAuthError::bad("invalid_request", "missing code").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
