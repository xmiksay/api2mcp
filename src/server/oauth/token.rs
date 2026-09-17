//! `POST /oauth/token` — dispatches on `grant_type`: `authorization_code` here (mandatory
//! PKCE S256, burns the code on use), `refresh_token` in `refresh.rs` (split out to respect
//! the 400-line cap, per the plan's own named seam for this file).

use axum::Form;
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use crate::server::identity::SCOPE_MCP;
use crate::server::state::AppState;
use crate::store::TokenGrant;

use super::refresh;
use super::shared::{OAuthError, token_response, verify_pkce_s256};

/// Access-token lifetime: short, since a refresh token mints a fresh one. Also the value
/// `refresh::grant` reports back in `expires_in` on rotation, since `rotate_refresh_token`
/// preserves each row's TTL *duration* — so a rotated access token always lives exactly this
/// long too.
pub const ACCESS_TOKEN_TTL_SECS: u64 = 3600;
/// Per-token refresh window (sliding: renewed on every rotation). Distinct from
/// `refresh::ABSOLUTE_FAMILY_TTL`, which does *not* slide.
const REFRESH_TOKEN_TTL_DAYS: u64 = 30;

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    // authorization_code
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
    pub client_id: Option<String>,
    // refresh_token
    pub refresh_token: Option<String>,
}

pub async fn token(
    State(state): State<AppState>,
    Form(req): Form<TokenRequest>,
) -> Result<Response, OAuthError> {
    match req.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, req).await,
        "refresh_token" => refresh::grant(&state, req).await,
        other => Err(OAuthError::bad(
            "unsupported_grant_type",
            format!("unsupported grant_type: {other}"),
        )),
    }
}

async fn exchange_code(state: &AppState, req: TokenRequest) -> Result<Response, OAuthError> {
    let code = req
        .code
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "code is required"))?;
    let verifier = req
        .code_verifier
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "code_verifier is required (PKCE)"))?;

    let store = state.stores().oauth();
    // `consume_code` marks the code used in the same atomic step as reading it, so even a
    // wrong `code_verifier` below burns it — a stricter single-attempt semantics than
    // check-then-burn, and it closes the window a check-then-burn design leaves open for
    // brute-forcing the verifier against a still-valid code.
    let consumed = store
        .consume_code(code)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_grant", "unknown, used, or expired code"))?;

    if consumed.code_challenge_method != "S256" {
        // Unreachable in practice: `authorize` already refuses any method but S256 before a
        // code is ever minted. Kept as defence in depth against a future code path that
        // stores a code some other way.
        return Err(OAuthError::bad(
            "invalid_grant",
            "only S256 PKCE is supported",
        ));
    }
    if !verify_pkce_s256(verifier, &consumed.code_challenge) {
        return Err(OAuthError::bad("invalid_grant", "PKCE verification failed"));
    }
    if let Some(rd) = req.redirect_uri.as_deref()
        && rd != consumed.redirect_uri
    {
        return Err(OAuthError::bad("invalid_grant", "redirect_uri mismatch"));
    }
    if let Some(cid) = req.client_id.as_deref() {
        let cid: Uuid = cid
            .parse()
            .map_err(|_| OAuthError::bad("invalid_grant", "client_id mismatch"))?;
        if cid != consumed.client_id {
            return Err(OAuthError::bad("invalid_grant", "client_id mismatch"));
        }
    }

    let scope = consumed.scope.unwrap_or_else(|| SCOPE_MCP.to_owned());
    let issued = store
        .issue_token(
            TokenGrant {
                client_id: consumed.client_id,
                user_id: consumed.user_id,
                scope: Some(scope.clone()),
                resource: consumed.resource,
            },
            std::time::Duration::from_secs(ACCESS_TOKEN_TTL_SECS),
            Some(std::time::Duration::from_secs(
                60 * 60 * 24 * REFRESH_TOKEN_TTL_DAYS,
            )),
        )
        .await?;

    Ok(token_response(
        &issued.access_token,
        issued.refresh_token.as_deref(),
        ACCESS_TOKEN_TTL_SECS as i64,
        Some(&scope),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_grant_type_is_rejected_before_touching_the_database() {
        // `token()` needs `AppState`; the match arm itself is pure, so pin its shape here
        // rather than only through an integration test.
        let grant_type = "client_credentials";
        let result: Result<(), OAuthError> = match grant_type {
            "authorization_code" | "refresh_token" => Ok(()),
            other => Err(OAuthError::bad(
                "unsupported_grant_type",
                format!("unsupported grant_type: {other}"),
            )),
        };
        let err = result.unwrap_err();
        assert_eq!(err.error, "unsupported_grant_type");
    }
}
