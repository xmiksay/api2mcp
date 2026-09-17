//! `POST /oauth/register` — RFC 7591 dynamic client registration, unauthenticated by design:
//! an MCP client (Claude Code) self-registers as the very first step of the OAuth flow,
//! before any user is logged in, so gating this on a session would make bootstrapping
//! impossible. Every registered client is public/PKCE-only — this server never hands out a
//! client secret, matching `token_endpoint_auth_methods_supported: ["none"]` in
//! `discovery::authorization_server_metadata`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::server::state::AppState;
use crate::store::NewOauthClient;

use super::shared::OAuthError;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
}

pub async fn register_client(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Response, OAuthError> {
    validate_redirect_uris(&req.redirect_uris)?;

    let client_name = req.client_name.unwrap_or_else(|| "mcp-client".to_owned());
    let grant_types = vec!["authorization_code".to_owned(), "refresh_token".to_owned()];

    let (client, _secret) = state
        .stores()
        .oauth()
        .register_client(
            NewOauthClient {
                client_name,
                redirect_uris: req.redirect_uris,
                grant_types,
                token_endpoint_auth_method: "none".to_owned(),
                scope: None,
            },
            false, // public client: PKCE only, never a secret.
        )
        .await?;

    let body = json!({
        "client_id": client.id.to_string(),
        "client_id_issued_at": client.created_at.timestamp(),
        "client_name": client.client_name,
        "redirect_uris": client.redirect_uris,
        "grant_types": client.grant_types,
        "response_types": ["code"],
        "token_endpoint_auth_method": client.token_endpoint_auth_method,
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// At least one `redirect_uri`, and every one of them a URL the client could plausibly
/// receive a redirect at — malformed input here becomes an unusable client, not a hazard
/// caught later, but there's no reason to let a definer typo through unnoticed.
fn validate_redirect_uris(uris: &[String]) -> Result<(), OAuthError> {
    if uris.is_empty() {
        return Err(OAuthError::bad(
            "invalid_redirect_uri",
            "redirect_uris is required and must be non-empty",
        ));
    }
    for uri in uris {
        if url::Url::parse(uri).is_err() {
            return Err(OAuthError::bad(
                "invalid_redirect_uri",
                format!("{uri:?} is not a valid URL"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_redirect_uris_is_rejected() {
        let err = validate_redirect_uris(&[]).unwrap_err();
        assert_eq!(err.error, "invalid_redirect_uri");
    }

    #[test]
    fn a_malformed_redirect_uri_is_rejected() {
        let err = validate_redirect_uris(&["not a url".to_owned()]).unwrap_err();
        assert_eq!(err.error, "invalid_redirect_uri");
    }

    #[test]
    fn well_formed_redirect_uris_pass() {
        assert!(validate_redirect_uris(&["http://localhost:9876/callback".to_owned()]).is_ok());
    }
}
