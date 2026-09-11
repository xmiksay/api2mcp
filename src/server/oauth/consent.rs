//! Server-rendered consent screen (`GET`/`POST /oauth/consent`) — one `format!`, no template
//! engine, same reasoning as `server::login`: this flow must work with only a `build.rs`
//! placeholder in `web/dist`.
//!
//! `oauth_consent_requests` carries no `user_id` column (see its migration's doc) — the
//! pending request is keyed only by client/redirect/PKCE/scope, and the row's own id (an
//! unguessable v4 UUID) is what makes the later POST forgery-proof: it must appear in both
//! the link the user clicked and the form body, and is deleted the instant it's consumed
//! either way (approve or deny), so it can never be replayed. The user who is logged in *at
//! POST time* is who the grant is recorded for and who the issued code belongs to.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::server::identity::SCOPE_MCP;
use crate::server::state::AppState;
use crate::store::ConsentRequest;

use super::authorize::issue_code_and_redirect;
use super::shared::{OAuthError, current_caller, percent_encode};

#[derive(Debug, Deserialize)]
pub struct ConsentQuery {
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConsentForm {
    pub request_id: String,
    pub decision: String,
}

pub async fn get_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ConsentQuery>,
) -> Result<Response, OAuthError> {
    let request_id = parse_request_id(q.request_id.as_deref())?;

    if current_caller(&state, &headers).await.is_none() {
        let next = format!("/oauth/consent?request_id={request_id}");
        return Ok(Redirect::to(&format!("/login?next={}", percent_encode(&next))).into_response());
    }

    let store = state.stores().oauth();
    let req = load_live_request(&store, request_id).await?;
    let client = store
        .get_client(req.client_id)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_client", "unknown client"))?;

    Ok(Html(render_page(request_id, &req, &client.client_name)).into_response())
}

pub async fn post_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConsentForm>,
) -> Result<Response, OAuthError> {
    let request_id = parse_request_id(Some(&form.request_id))?;

    let Some(caller) = current_caller(&state, &headers).await else {
        return Err(OAuthError::bad("access_denied", "not logged in"));
    };

    let store = state.stores().oauth();
    let req = load_live_request(&store, request_id).await?;
    // Single-use from here regardless of outcome: an approval or denial both consume it, so
    // it can never be replayed to mint a second code.
    store.delete_consent_request(request_id).await?;

    if form.decision != "approve" {
        return Ok(deny_redirect(&req));
    }

    let scope = req.scope.clone().unwrap_or_else(|| SCOPE_MCP.to_owned());
    store
        .grant_consent(caller.id, req.client_id, &scope)
        .await?;

    issue_code_and_redirect(
        &state,
        req.client_id,
        caller.id,
        &req.redirect_uri,
        &req.code_challenge,
        &req.code_challenge_method,
        &scope,
        req.resource.clone(),
        req.state.as_deref(),
    )
    .await
}

fn parse_request_id(raw: Option<&str>) -> Result<Uuid, OAuthError> {
    raw.and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| OAuthError::bad("invalid_request", "missing or malformed request_id"))
}

/// Loads a pending consent request, treating an already-expired row the same as a missing
/// one (deleting it first so it can't linger and be mistaken for live by a later lookup).
async fn load_live_request(
    store: &crate::store::OauthStore,
    request_id: Uuid,
) -> Result<ConsentRequest, OAuthError> {
    let Some(req) = store.get_consent_request(request_id).await? else {
        return Err(OAuthError::bad(
            "invalid_request",
            "consent request not found or already used",
        ));
    };
    if req.expires_at <= chrono::Utc::now() {
        store.delete_consent_request(request_id).await?;
        return Err(OAuthError::bad(
            "invalid_request",
            "consent request expired",
        ));
    }
    Ok(req)
}

fn deny_redirect(req: &ConsentRequest) -> Response {
    let sep = if req.redirect_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut target = format!("{}{sep}error=access_denied", req.redirect_uri);
    if let Some(st) = req.state.as_deref().filter(|s| !s.is_empty()) {
        target.push_str(&format!("&state={}", percent_encode(st)));
    }
    Redirect::to(&target).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_page(request_id: Uuid, req: &ConsentRequest, client_name: &str) -> String {
    let scope = req.scope.as_deref().unwrap_or(SCOPE_MCP);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Authorize — api2mcp</title>
<style>
  body {{ font-family: system-ui, sans-serif; max-width: 26rem; margin: 4rem auto; padding: 0 1rem; }}
  .client {{ font-weight: 600; }}
  .scope {{ color: #555; font-size: 0.9rem; margin-bottom: 1.5rem; }}
  .buttons {{ display: flex; gap: 0.75rem; }}
  button {{ padding: 0.5rem 1rem; font-size: 1rem; }}
  button[value="deny"] {{ background: none; }}
</style>
</head>
<body>
<h1>Authorize access</h1>
<p><span class="client">{client_name}</span> wants to access your api2mcp tools.</p>
<p class="scope">Requested scope: <code>{scope}</code></p>
<form method="post" action="/oauth/consent">
  <input type="hidden" name="request_id" value="{request_id}">
  <div class="buttons">
    <button type="submit" name="decision" value="approve">Allow</button>
    <button type="submit" name="decision" value="deny">Deny</button>
  </div>
</form>
</body>
</html>
"#,
        client_name = html_escape(client_name),
        scope = html_escape(scope),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_id_rejects_malformed_input() {
        assert!(parse_request_id(Some("not-a-uuid")).is_err());
        assert!(parse_request_id(None).is_err());
    }

    #[test]
    fn parse_request_id_accepts_a_valid_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(parse_request_id(Some(&id.to_string())).unwrap(), id);
    }

    #[test]
    fn render_page_escapes_a_hostile_client_name() {
        let req = ConsentRequest {
            id: Uuid::new_v4(),
            client_id: Uuid::new_v4(),
            redirect_uri: "https://client.example/cb".to_owned(),
            scope: Some("mcp".to_owned()),
            resource: None,
            code_challenge: "abc".to_owned(),
            code_challenge_method: "S256".to_owned(),
            state: None,
            expires_at: chrono::Utc::now(),
        };
        let html = render_page(req.id, &req, "<script>steal()</script>");
        assert!(!html.contains("<script>steal()"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
