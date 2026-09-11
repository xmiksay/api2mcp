//! `GET /oauth/authorize` — validates the request, requires a logged-in user (bounces to
//! `/login?next=…` otherwise, which `server::login` already validates), and either takes the
//! remembered-consent fast path or parks a consent request and sends the user to the
//! server-rendered consent screen.

use axum::extract::{OriginalUri, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::server::identity::SCOPE_MCP;
use crate::server::state::AppState;
use crate::store::{NewConsentRequest, NewOauthCode};

use super::shared::{OAuthError, current_caller, percent_encode};

/// Authorization-code lifetime — short, single-use.
const CODE_TTL_SECS: i64 = 600;
/// Pending-consent-request lifetime — short, single-use (the row's own id doubles as the
/// consent screen's CSRF token, see `consent`'s module doc).
const CONSENT_REQUEST_TTL_SECS: i64 = 600;

#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub scope: Option<String>,
    pub state: Option<String>,
    /// RFC 8707 resource indicator — carried through the code and into the issued token
    /// rather than dropped (the MCP spec has clients send this).
    pub resource: Option<String>,
}

pub async fn authorize(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Response, OAuthError> {
    if q.response_type.as_deref() != Some("code") {
        return Err(OAuthError::bad(
            "unsupported_response_type",
            "only response_type=code is supported",
        ));
    }
    let method = q.code_challenge_method.as_deref().unwrap_or("S256");
    if method != "S256" {
        return Err(OAuthError::bad(
            "invalid_request",
            "only code_challenge_method=S256 is supported (PKCE \"plain\" is refused)",
        ));
    }
    let challenge = q
        .code_challenge
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| OAuthError::bad("invalid_request", "code_challenge is required (PKCE)"))?;
    let client_id_str = q
        .client_id
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "client_id is required"))?;
    // Unknown/malformed client_id: there is no trusted redirect target yet, so fail directly
    // rather than redirecting to an attacker-supplied URL.
    let client_id: Uuid = client_id_str
        .parse()
        .map_err(|_| OAuthError::bad("invalid_client", "unknown client_id"))?;
    let redirect_uri = q
        .redirect_uri
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "redirect_uri is required"))?;

    let store = state.stores().oauth();
    let client = store
        .get_client(client_id)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_client", "unknown client_id"))?;
    if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return Err(OAuthError::bad(
            "invalid_request",
            "redirect_uri is not registered for this client",
        ));
    }

    // From here on a failure can redirect to `redirect_uri` per RFC 6749 §4.1.2.1 — but we
    // keep returning direct errors for the caller's own mistakes below too, matching the
    // reference implementation this module was ported from: only a *user decision*
    // (consent denied) redirects with `error=`, everything else fails closed and visibly.
    let Some(caller) = current_caller(&state, &headers).await else {
        let next = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        return Ok(Redirect::to(&format!("/login?next={}", percent_encode(next))).into_response());
    };

    let scope = q.scope.clone().unwrap_or_else(|| SCOPE_MCP.to_owned());

    // A prior approval of this exact (user, client, scope) triple skips the consent screen —
    // this is what keeps re-auth a one-shot approval rather than nagging on every sign-in.
    if store.has_consent(caller.id, client_id, &scope).await? {
        return issue_code_and_redirect(
            &state,
            client_id,
            caller.id,
            redirect_uri,
            challenge,
            method,
            &scope,
            q.resource.clone(),
            q.state.as_deref(),
        )
        .await;
    }

    // First time this user is asked about this client: park the request and send them to the
    // consent screen. The request's own id is only ever handed out inside this redirect, so
    // it is what makes the screen's later POST forgery-proof (see `consent`'s module doc).
    let id = store
        .create_consent_request(NewConsentRequest {
            client_id,
            redirect_uri: redirect_uri.to_owned(),
            scope: Some(scope),
            resource: q.resource.clone(),
            code_challenge: challenge.to_owned(),
            code_challenge_method: method.to_owned(),
            state: q.state.clone(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(CONSENT_REQUEST_TTL_SECS),
        })
        .await?;

    Ok(Redirect::to(&format!("/oauth/consent?request_id={id}")).into_response())
}

/// Mints an authorization code and builds the redirect back to the client. The shared tail of
/// the authorize-with-remembered-consent path and the consent-screen approve path (`consent`)
/// — both end up here so the code-minting logic lives exactly once.
#[allow(clippy::too_many_arguments)]
pub(super) async fn issue_code_and_redirect(
    state: &AppState,
    client_id: Uuid,
    user_id: Uuid,
    redirect_uri: &str,
    code_challenge: &str,
    code_challenge_method: &str,
    scope: &str,
    resource: Option<String>,
    oauth_state: Option<&str>,
) -> Result<Response, OAuthError> {
    let code = state
        .stores()
        .oauth()
        .create_code(NewOauthCode {
            client_id,
            user_id,
            redirect_uri: redirect_uri.to_owned(),
            code_challenge: code_challenge.to_owned(),
            code_challenge_method: code_challenge_method.to_owned(),
            resource,
            scope: Some(scope.to_owned()),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(CODE_TTL_SECS),
        })
        .await?;

    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut target = format!("{redirect_uri}{sep}code={}", percent_encode(&code));
    if let Some(st) = oauth_state.filter(|s| !s.is_empty()) {
        target.push_str(&format!("&state={}", percent_encode(st)));
    }
    Ok(Redirect::to(&target).into_response())
}
