//! `GET /login/oidc/start` and `GET /login/oidc/callback` — the browser-facing half of Decision
//! 2's OIDC login. [`super::oidc`] talks to the provider; this module owns the `state`/PKCE
//! flow cookie and turns a successful callback into the same session cookie
//! [`super::login::post_login`] already issues (via [`super::login::set_cookie_header`]) —
//! everything downstream of that point is unchanged.
//!
//! **The flow cookie (`a2m_oidc_flow`) is the CSRF/PKCE anchor.** `start` picks a random
//! `state` and a PKCE verifier, sends the provider only their public halves (`state` itself,
//! and the S256 `code_challenge`), and stashes both privately in an `HttpOnly` cookie scoped to
//! `/login/oidc`. `callback` requires the query string's `state` to equal the cookie's — an
//! attacker who can make a victim's browser hit the callback URL (no `HttpOnly` cookie access
//! needed for that; it's just a link) cannot forge the cookie itself, so cannot complete the
//! exchange as the victim. The cookie is single-use: every path out of `callback` clears it,
//! successful or not, via [`clear_flow_cookie`].
//!
//! On any failure, the callback never explains *why* to the browser (proxying userinfo/token
//! errors into a redirect target would let a malicious provider or network error inject text
//! into a page this crate serves) — it always redirects to the same generic
//! `/login?error=oidc`, and every reason is logged server-side instead.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::config::{Config, OidcConfig};
use crate::store::UserStore;

use super::login::{set_cookie_header, validate_next};
use super::{auth, oidc};

const OIDC_FLOW_COOKIE_NAME: &str = "a2m_oidc_flow";
const FLOW_TTL_SECS: u64 = 600;

#[derive(Debug, Deserialize)]
pub struct OidcCallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// Present when the provider itself denied or errored the request (RFC 6749 §4.1.2.1) —
    /// its value is never shown to the browser, only logged.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FlowState {
    state: String,
    verifier: String,
    next: String,
}

/// `GET /login/oidc/start` — discovers the provider, mints `state`/PKCE, stashes them in the
/// flow cookie, and redirects to the provider's authorization endpoint.
pub async fn get_oidc_start(cfg: &Config, oidc_cfg: &OidcConfig, next: Option<&str>) -> Response {
    let next = validate_next(next).unwrap_or("/").to_owned();
    let client = reqwest::Client::new();
    let discovery = match oidc::discover(&client, &oidc_cfg.issuer).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "oidc start: discovery failed");
            return login_failed_redirect(&next);
        }
    };
    let pkce = oidc::generate_pkce();
    let state = auth::new_token();
    let authorize_url = match oidc::authorize_url(&discovery, oidc_cfg, &state, &pkce.challenge) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "oidc start: building the authorize url failed");
            return login_failed_redirect(&next);
        }
    };

    let flow = FlowState {
        state,
        verifier: pkce.verifier,
        next,
    };
    let mut response = Redirect::to(authorize_url.as_str()).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        flow_cookie_header(cfg, &encode_flow(&flow), FLOW_TTL_SECS),
    );
    response
}

/// `GET /login/oidc/callback` — verifies `state`, exchanges the code (checking the PKCE
/// verifier), resolves the identity via userinfo, and issues the same session cookie the
/// password path does. See this module's own doc for the failure-handling shape.
pub async fn get_oidc_callback(
    db: &DatabaseConnection,
    cfg: &Config,
    oidc_cfg: &OidcConfig,
    query: OidcCallbackQuery,
    cookie_header: Option<&str>,
) -> Response {
    let Some(flow) = cookie_header
        .and_then(|h| auth::cookie_value(h, OIDC_FLOW_COOKIE_NAME))
        .and_then(decode_flow)
    else {
        tracing::warn!("oidc callback: missing or unreadable flow cookie");
        return clear_flow_and_fail(cfg, "/");
    };

    if let Some(err) = query.error {
        tracing::warn!(provider_error = %err, "oidc callback: provider returned an error");
        return clear_flow_and_fail(cfg, &flow.next);
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        tracing::warn!("oidc callback: missing code or state");
        return clear_flow_and_fail(cfg, &flow.next);
    };
    // `state` is a single-use, unguessable, server-issued token bound to this one cookie, not a
    // secret checked against high-volume attacker input the way a password is — a plain `!=` is
    // the right tool here, not a constant-time comparison.
    if returned_state != flow.state {
        tracing::warn!("oidc callback: state parameter did not match the flow cookie");
        return clear_flow_and_fail(cfg, &flow.next);
    }

    let client = reqwest::Client::new();
    let discovery = match oidc::discover(&client, &oidc_cfg.issuer).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "oidc callback: discovery failed");
            return clear_flow_and_fail(cfg, &flow.next);
        }
    };
    let access_token =
        match oidc::exchange_code(&client, &discovery, oidc_cfg, &code, &flow.verifier).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "oidc callback: code exchange failed");
                return clear_flow_and_fail(cfg, &flow.next);
            }
        };
    let identity = match oidc::fetch_identity(&client, &discovery, &access_token).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "oidc callback: fetching the identity failed");
            return clear_flow_and_fail(cfg, &flow.next);
        }
    };
    let user = match UserStore::new(db.clone())
        .find_or_create_by_oidc(
            &oidc_cfg.issuer,
            &identity.subject,
            &identity.email,
            identity.email_verified,
        )
        .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "oidc callback: resolving the user failed");
            return clear_flow_and_fail(cfg, &flow.next);
        }
    };
    let cookie_value = match auth::create_session(db, user.id, cfg.session_ttl).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "oidc callback: session creation failed");
            return clear_flow_and_fail(cfg, &flow.next);
        }
    };

    let mut response = Redirect::to(&flow.next).into_response();
    let headers = response.headers_mut();
    headers.append(
        header::SET_COOKIE,
        set_cookie_header(cfg, &cookie_value, cfg.session_ttl.as_secs()),
    );
    headers.append(header::SET_COOKIE, clear_flow_cookie(cfg));
    response
}

fn login_failed_redirect(next: &str) -> Response {
    let target = format!(
        "/login?error=oidc&next={}",
        percent_encoding::utf8_percent_encode(next, percent_encoding::NON_ALPHANUMERIC)
    );
    Redirect::to(&target).into_response()
}

fn clear_flow_and_fail(cfg: &Config, next: &str) -> Response {
    let mut response = login_failed_redirect(next);
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_flow_cookie(cfg));
    response
}

fn clear_flow_cookie(cfg: &Config) -> HeaderValue {
    flow_cookie_header(cfg, "", 0)
}

/// Same `HttpOnly`/`SameSite=Lax`/conditional-`Secure` shape as
/// [`super::login::set_cookie_header`], scoped to `Path=/login/oidc` (the only two routes that
/// ever need to see it) instead of `/`.
fn flow_cookie_header(cfg: &Config, value: &str, max_age_secs: u64) -> HeaderValue {
    let secure = if cfg.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let raw = format!(
        "{OIDC_FLOW_COOKIE_NAME}={value}; Path=/login/oidc; HttpOnly; SameSite=Lax; \
         Max-Age={max_age_secs}{secure}"
    );
    HeaderValue::from_str(&raw)
        .unwrap_or_else(|_| HeaderValue::from_static("a2m_oidc_flow=; Path=/login/oidc; Max-Age=0"))
}

fn encode_flow(flow: &FlowState) -> String {
    // `FlowState` is three plain strings — this cannot fail in practice, and there is no
    // sensible fallback for a `Set-Cookie` value that failed to encode, so this is one of the
    // few places a `String` default (an empty flow, which just fails the next request's
    // `decode_flow`) is the right degrade rather than plumbing a `Result` through `get_oidc_start`.
    serde_json::to_vec(flow)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .unwrap_or_default()
}

fn decode_flow(raw: &str) -> Option<FlowState> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            database_url: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            base_url: "http://test.local:8080".into(),
            seed_email: None,
            seed_password: None,
            run_retention_days: 30,
            allow_loopback_upstream: false,
            session_ttl: std::time::Duration::from_secs(3600),
            max_request_bytes: 1024 * 1024,
            oidc: None,
        }
    }

    #[test]
    fn flow_state_round_trips_through_encode_decode() {
        let flow = FlowState {
            state: "s1".into(),
            verifier: "v1".into(),
            next: "/runs".into(),
        };
        let encoded = encode_flow(&flow);
        let decoded = decode_flow(&encoded).expect("round trips");
        assert_eq!(decoded.state, "s1");
        assert_eq!(decoded.verifier, "v1");
        assert_eq!(decoded.next, "/runs");
    }

    #[test]
    fn decode_flow_rejects_garbage() {
        assert!(decode_flow("not valid base64url json").is_none());
    }

    #[test]
    fn flow_cookie_header_is_http_only_and_scoped_to_login_oidc() {
        let header = flow_cookie_header(&cfg(), "abc", 600);
        let rendered = header.to_str().expect("ascii");
        assert!(rendered.contains("HttpOnly"));
        assert!(rendered.contains("Path=/login/oidc"));
        assert!(rendered.contains("Max-Age=600"));
    }
}
