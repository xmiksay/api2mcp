//! Server-rendered login: `GET /login`, `POST /login`, `GET /logout`. The OIDC half of the
//! flow — `GET /login/oidc/start` and `GET /login/oidc/callback` (Decision 2) — lives in
//! [`super::login_oidc`], a separate module purely to keep this file under the workspace's
//! 400-line cap; conceptually it's the same "server-rendered login" surface, and `GET /login`
//! below is what decides whether to offer it at all.
//!
//! **This is deliberately not an SPA view**, and there is no `LoginView.vue`. Two
//! independent reasons, either one would be enough on its own:
//!
//! 1. `/oauth/authorize` (chunk C12) bounces an unauthenticated user here mid-flow. If
//!    login were an SPA route, the *entire* auth path — including the OAuth AS, which has
//!    nothing to do with the admin UI — would depend on `web/dist` holding a real build.
//!    `web/dist` is a `build.rs` placeholder on a fresh clone and in CI (`SKIP_UI_BUILD=1`,
//!    see `docs/architecture.md`), so an SPA login would make the whole auth flow depend on
//!    a bundle that often simply isn't there.
//! 2. Server-rendering keeps the session token out of reach of JavaScript entirely: it
//!    only ever exists as an `HttpOnly` `Set-Cookie` header and a form POST body, never as
//!    a value any client-side script could read, log, or send somewhere it shouldn't.
//!
//! Two small pages don't earn a template engine — the HTML is one `format!` away.
//!
//! Handlers here take plain arguments (`&DatabaseConnection`, `&Config`, request data
//! already extracted into plain structs) rather than axum's `State`/`Form`/`Query`
//! extractors bound to a concrete state type: `AppState` is chunk C11's, and doesn't exist
//! in this chunk. C11 wraps each function below in a one-line real handler that does the
//! axum-specific extraction and calls straight through.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::config::Config;
use crate::store::UserStore;

use super::auth;

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    pub next: Option<String>,
    /// Presence (any value, including empty) means `super::login_oidc` bounced back here after
    /// a failed OIDC attempt — see that module's doc for why the failure detail itself never
    /// survives the redirect. Not surfaced anywhere except this one generic message.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub next: Option<String>,
}

/// `GET /login` — renders the password form, plus a link to the OIDC provider when one is
/// configured (`cfg.oidc.is_some()`) — the fallback Decision 2 requires: a deployment with no
/// provider configured still has a way in via the password form alone.
pub fn get_login(cfg: &Config, next: Option<&str>, oidc_failed: bool) -> Response {
    let next = validate_next(next).unwrap_or("/");
    let error = oidc_failed.then_some("sign-in with the identity provider failed, try again");
    Html(render_login_page(cfg, next, error)).into_response()
}

/// Shared by [`get_login`] and [`post_login`]'s failure paths: every rendering of the login
/// page offers the same OIDC link (or none), so there is exactly one place that decides
/// whether to show it.
fn render_login_page(cfg: &Config, next: &str, error: Option<&str>) -> String {
    let oidc_link = cfg
        .oidc
        .as_ref()
        .map(|o| (provider_label(&o.issuer), oidc_start_url(next)));
    render_page(next, error, oidc_link.as_ref())
}

/// A short, human-readable label for the provider link — the issuer's own hostname, since
/// Decision 3 deliberately adds no separate "display name" env var for this. Falls back to the
/// generic "identity provider" for a malformed issuer, which `Config::validate` already makes
/// unreachable in practice (an issuer without a scheme never gets this far).
fn provider_label(issuer: &str) -> String {
    url::Url::parse(issuer)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "identity provider".to_owned())
}

fn oidc_start_url(next: &str) -> String {
    format!(
        "/login/oidc/start?next={}",
        percent_encoding::utf8_percent_encode(next, percent_encoding::NON_ALPHANUMERIC)
    )
}

/// `POST /login` — argon2 verify, set the session cookie, redirect to `next` (or `/`).
/// Failure re-renders the same form with one generic message: this method must not let a
/// caller distinguish "no such user" from "wrong password" any more than
/// [`crate::store::UserStore::verify_password`] already refuses to.
pub async fn post_login(db: &DatabaseConnection, cfg: &Config, form: LoginForm) -> Response {
    let next = validate_next(form.next.as_deref())
        .unwrap_or("/")
        .to_owned();

    let user = match UserStore::new(db.clone())
        .verify_password(&form.email, &form.password)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Html(render_login_page(
                    cfg,
                    &next,
                    Some("invalid email or password"),
                )),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "login: password verification failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(render_login_page(
                    cfg,
                    &next,
                    Some("something went wrong, try again"),
                )),
            )
                .into_response();
        }
    };

    let cookie_value = match auth::create_session(db, user.id, cfg.session_ttl).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "login: session creation failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(render_login_page(
                    cfg,
                    &next,
                    Some("something went wrong, try again"),
                )),
            )
                .into_response();
        }
    };

    let mut response = Redirect::to(&next).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        set_cookie_header(cfg, &cookie_value, cfg.session_ttl.as_secs()),
    );
    response
}

/// `GET /logout` — best-effort session delete, clears the cookie, redirects to `/login`.
/// Idempotent: calling it with no session cookie, or a stale one, still redirects cleanly.
pub async fn get_logout(
    db: &DatabaseConnection,
    cfg: &Config,
    cookie_header: Option<&str>,
) -> Response {
    if let Some(token) =
        cookie_header.and_then(|h| auth::cookie_value(h, auth::SESSION_COOKIE_NAME))
        && let Err(e) = auth::delete_session(db, token).await
    {
        tracing::warn!(error = %e, "logout: failed to delete session row");
    }

    let mut response = Redirect::to("/login").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, set_cookie_header(cfg, "", 0));
    response
}

/// Builds the `Set-Cookie` value: `HttpOnly`, `SameSite=Lax` always; `Secure` whenever
/// `cfg.base_url` is `https://` — checked against the server's own configured origin
/// rather than a per-request, spoofable `X-Forwarded-Proto` header, since this decision is
/// security-relevant (an attacker who can flip it downgrades the cookie).
///
/// `pub(super)`: `super::login_oidc` sets this exact session cookie on a successful callback,
/// same as the password path does here — one function, so the two can never drift apart.
pub(super) fn set_cookie_header(cfg: &Config, value: &str, max_age_secs: u64) -> HeaderValue {
    let secure = if cfg.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let raw = format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}",
        auth::SESSION_COOKIE_NAME
    );
    // `value` is either empty (clearing the cookie) or `auth::new_token()`'s base64url
    // output, and every other component is a fixed literal — always valid header bytes.
    HeaderValue::from_str(&raw)
        .unwrap_or_else(|_| HeaderValue::from_static("a2m_session=; Path=/; Max-Age=0"))
}

/// Accepts only a same-site absolute path: starts with a single `/`, not `//` (a
/// protocol-relative URL), not `/\` (some browsers treat a leading backslash as a second
/// slash), and contains no `://` (an absolute URL with a scheme). Anything else falls back
/// to `/` at the call site rather than being an error — a malformed `next=` should degrade
/// to "go to the dashboard", not fail the login.
pub fn validate_next(next: Option<&str>) -> Option<&str> {
    let n = next?.trim();
    let looks_safe =
        n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\") && !n.contains("://");
    looks_safe.then_some(n)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `oidc_link` is `Some((provider_label, start_url))` when a provider is configured — rendered
/// above the password form with a divider, never in its place (Decision 2's fallback).
fn render_page(next: &str, error: Option<&str>, oidc_link: Option<&(String, String)>) -> String {
    let error_html = match error {
        Some(e) => format!(r#"<p class="error">{}</p>"#, html_escape(e)),
        None => String::new(),
    };
    let oidc_html = match oidc_link {
        Some((label, url)) => format!(
            r#"<a class="oidc-btn" href="{}">Sign in with {}</a><p class="divider">or</p>"#,
            html_escape(url),
            html_escape(label)
        ),
        None => String::new(),
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sign in — api2mcp</title>
<link rel="stylesheet" href="/static/auth.css">
</head>
<body>
<main>
<h1>api2mcp</h1>
<p class="sub">Sign in to manage your endpoints</p>
{error_html}
{oidc_html}
<form method="post" action="/login">
  <input type="hidden" name="next" value="{next}">
  <label>Email<input type="email" name="email" required autofocus></label>
  <label>Password<input type="password" name="password" required></label>
  <button type="submit">Sign in</button>
</form>
</main>
</body>
</html>
"#,
        next = html_escape(next),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OidcConfig;
    use crate::secret::Secret;

    fn cfg() -> Config {
        Config {
            database_url: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            base_url: "http://test.local:8080".into(),
            default_endpoint: "default".into(),
            seed_email: None,
            seed_password: None,
            run_retention_days: 30,
            allow_loopback_upstream: false,
            session_ttl: std::time::Duration::from_secs(3600),
            max_request_bytes: 1024 * 1024,
            oidc: None,
        }
    }

    fn cfg_with_oidc() -> Config {
        Config {
            oidc: Some(OidcConfig {
                issuer: "https://idp.example.com".into(),
                client_id: "client-1".into(),
                client_secret: Secret::from_raw("shh".into()),
                redirect_uri: "http://test.local:8080/login/oidc/callback".into(),
            }),
            ..cfg()
        }
    }

    #[test]
    fn validate_next_accepts_a_same_site_path() {
        assert_eq!(validate_next(Some("/runs")), Some("/runs"));
    }

    #[test]
    fn validate_next_rejects_protocol_relative() {
        assert_eq!(validate_next(Some("//evil.com")), None);
    }

    #[test]
    fn validate_next_rejects_an_absolute_url() {
        assert_eq!(validate_next(Some("https://evil.com")), None);
    }

    #[test]
    fn validate_next_rejects_backslash_trick_and_embedded_scheme() {
        assert_eq!(validate_next(Some("/\\evil.com")), None);
        assert_eq!(validate_next(Some("/x?y=http://evil.com")), None);
    }

    #[test]
    fn validate_next_rejects_missing_or_relative() {
        assert_eq!(validate_next(None), None);
        assert_eq!(validate_next(Some("runs")), None);
    }

    #[test]
    fn render_page_escapes_a_reflected_error_and_next() {
        let html = render_page(
            "/a\"onmouseover=alert(1)",
            Some("<script>steal()</script>"),
            None,
        );
        assert!(!html.contains("<script>steal()"));
        assert!(!html.contains(r#""onmouseover"#));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn render_page_omits_the_oidc_link_when_none() {
        let html = render_page("/runs", None, None);
        // The `.oidc-btn` rule lives in the linked stylesheet regardless; what must be absent
        // here is an element actually using it.
        assert!(!html.contains(r#"class="oidc-btn""#));
    }

    #[test]
    fn render_page_shows_the_oidc_link_when_configured() {
        let html = render_page(
            "/runs",
            None,
            Some(&("idp.example.com".to_owned(), "/login/oidc/start".to_owned())),
        );
        assert!(html.contains(r#"class="oidc-btn""#));
        assert!(html.contains("idp.example.com"));
        assert!(html.contains("/login/oidc/start"));
    }

    #[test]
    fn get_login_renders_the_form() {
        let response = get_login(&cfg(), Some("/runs"), false);
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn get_login_offers_the_provider_when_configured() {
        let response = get_login(&cfg_with_oidc(), Some("/runs"), false);
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn provider_label_uses_the_issuer_host() {
        assert_eq!(
            provider_label("https://idp.example.com/"),
            "idp.example.com"
        );
    }

    #[test]
    fn provider_label_falls_back_on_a_malformed_issuer() {
        assert_eq!(provider_label("not a url"), "identity provider");
    }

    #[test]
    fn oidc_start_url_percent_encodes_next() {
        assert_eq!(
            oidc_start_url("/runs?x=1"),
            "/login/oidc/start?next=%2Fruns%3Fx%3D1"
        );
    }
}
