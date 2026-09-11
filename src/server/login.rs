//! Server-rendered login: `GET /login`, `POST /login`, `GET /logout`.
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
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub next: Option<String>,
}

/// `GET /login` — renders the form, honouring a validated `next=`.
pub fn get_login(next: Option<&str>) -> Response {
    let next = validate_next(next).unwrap_or("/");
    Html(render_page(next, None)).into_response()
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
                Html(render_page(&next, Some("invalid email or password"))),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "login: password verification failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(render_page(&next, Some("something went wrong, try again"))),
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
                Html(render_page(&next, Some("something went wrong, try again"))),
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
fn set_cookie_header(cfg: &Config, value: &str, max_age_secs: u64) -> HeaderValue {
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

fn render_page(next: &str, error: Option<&str>) -> String {
    let error_html = match error {
        Some(e) => format!(r#"<p class="error">{}</p>"#, html_escape(e)),
        None => String::new(),
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sign in — api2mcp</title>
<style>
  body {{ font-family: system-ui, sans-serif; max-width: 22rem; margin: 4rem auto; padding: 0 1rem; }}
  label {{ display: block; margin-bottom: 1rem; font-size: 0.9rem; }}
  input {{ display: block; width: 100%; padding: 0.5rem; margin-top: 0.25rem; box-sizing: border-box; font-size: 1rem; }}
  button {{ padding: 0.5rem 1rem; font-size: 1rem; }}
  .error {{ color: #b00020; }}
</style>
</head>
<body>
<h1>api2mcp</h1>
{error_html}
<form method="post" action="/login">
  <input type="hidden" name="next" value="{next}">
  <label>Email<input type="email" name="email" required autofocus></label>
  <label>Password<input type="password" name="password" required></label>
  <button type="submit">Sign in</button>
</form>
</body>
</html>
"#,
        next = html_escape(next),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let html = render_page("/a\"onmouseover=alert(1)", Some("<script>steal()</script>"));
        assert!(!html.contains("<script>steal()"));
        assert!(!html.contains(r#""onmouseover"#));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn get_login_renders_the_form() {
        let response = get_login(Some("/runs"));
        assert_eq!(response.status(), StatusCode::OK);
    }
}
