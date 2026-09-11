//! Sends a [`BoundRequest`], following redirects manually with a hard hop cap.
//!
//! Every hop re-runs [`check_url`] and re-applies auth — not just hop zero. Two reasons converge
//! on the same mechanism: `reqwest`'s clients are built with `redirect::Policy::none()` (see
//! `http::client`), so a 3xx would otherwise just come back as a response for us to interpret;
//! and because auth is origin-bound (I5), re-applying `http::auth::apply` on every hop is what
//! stops a cross-origin redirect from carrying the first origin's credential along — closing SSRF
//! and credential leakage with the same loop.
//!
//! A relative `Location` is resolved with `Url::join` against the *current* URL — unlike in
//! `http::bind`, that's correct here, because the joined result goes straight back through
//! `check_url` before anything is sent to it.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::model::{AuthProvider, Origin};

use super::GuardError;
use super::auth::AuthError;
use super::bind::BoundRequest;
use super::body::{BodyError, read_capped};
use super::redact::redact_message;
use super::ssrf::{SsrfPolicy, check_url};

/// Per-call knobs `send` needs that aren't a property of the request itself. C7 (budgets) will
/// derive `deadline` and `max_response_bytes` from a folded `Budgets`; here they're plain
/// parameters, same as the plan calls for. Every field is a reference or `Copy`, so the whole
/// struct is `Copy` — `http::paginate` reuses one value across every page's call.
#[derive(Clone, Copy)]
pub struct SendParams<'a> {
    pub allowlist: &'a BTreeSet<Origin>,
    pub policy: &'a SsrfPolicy,
    pub auth: Option<&'a AuthProvider>,
    pub max_response_bytes: u64,
    /// Wall-clock budget for the *whole* call, redirects included.
    pub deadline: Duration,
    pub max_redirects: u8,
}

/// A completed response: the final hop's status/headers/body, and the URL that hop was sent to
/// (already origin-checked — `http::redact::redact_url` is the caller's job before persisting or
/// returning it).
#[derive(Debug, Clone)]
pub struct CallResponse {
    pub status: ::http::StatusCode,
    pub headers: ::http::HeaderMap,
    pub url: url::Url,
    pub body: Vec<u8>,
}

/// Model-visible errors from `send`. `thiserror` + `Serialize`, never `anyhow` — a raw transport
/// error can embed a URL with userinfo or a query-string credential, so every string-carrying
/// variant here is built from `http::redact::redact_message`, never a bare `.to_string()`.
#[derive(Debug, Clone, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum CallError {
    #[error("guard: {0}")]
    Guard(#[from] GuardError),
    #[error("auth: {0}")]
    Auth(#[from] AuthError),
    #[error("body: {0}")]
    Body(#[from] BodyError),
    #[error("more than {max} redirects")]
    TooManyRedirects { max: u8 },
    #[error("redirect response carried no Location header")]
    MissingLocation,
    #[error("redirect Location could not be resolved to a URL: {reason}")]
    InvalidLocation { reason: String },
    #[error("request timed out")]
    Timeout,
    #[error("a header could not be represented as an HTTP header")]
    InvalidHeader,
    #[error("sending request: {message}")]
    Transport { message: String },
}

/// Sends `request`, following redirects (301/302/303/307/308) up to `params.max_redirects`
/// times. See the module docs for why every hop re-runs the guard and auth from scratch.
pub async fn send(
    client: &reqwest::Client,
    mut request: BoundRequest,
    params: SendParams<'_>,
) -> Result<CallResponse, CallError> {
    let started = Instant::now();
    let mut hop: u8 = 0;

    loop {
        check_url(&request.url, params.allowlist, params.policy)?;

        let remaining = params
            .deadline
            .checked_sub(started.elapsed())
            .ok_or(CallError::Timeout)?;

        let mut headers = header_map(&request.headers)?;
        if let Some(provider) = params.auth {
            super::auth::apply(provider, &request.url, &mut headers)?;
        }

        let mut builder = client
            .request(request.method.clone(), request.url.clone())
            .headers(headers)
            .timeout(remaining);
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }

        let response = builder.send().await.map_err(map_transport_err)?;
        let status = response.status();

        if is_redirect(status) {
            hop += 1;
            if hop > params.max_redirects {
                return Err(CallError::TooManyRedirects {
                    max: params.max_redirects,
                });
            }
            let location = response
                .headers()
                .get(::http::header::LOCATION)
                .ok_or(CallError::MissingLocation)?
                .to_str()
                .map_err(|_| CallError::InvalidLocation {
                    reason: "Location header is not valid ASCII".to_owned(),
                })?;
            let next_url = request
                .url
                .join(location)
                .map_err(|e| CallError::InvalidLocation {
                    reason: e.to_string(),
                })?;
            let (next_method, next_body) = downgrade(&request.method, request.body, status);
            request = BoundRequest {
                method: next_method,
                url: next_url,
                headers: request.headers,
                body: next_body,
            };
            continue;
        }

        let out_headers = response.headers().clone();
        let final_url = request.url.clone();
        let body = read_capped(response, params.max_response_bytes).await?;
        return Ok(CallResponse {
            status,
            headers: out_headers,
            url: final_url,
            body,
        });
    }
}

fn is_redirect(status: ::http::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// 301/302/303 on a non-GET method downgrade to GET and drop the body; 307/308 always preserve
/// both, and a GET staying GET has no body to drop in the first place.
fn downgrade(
    method: &::http::Method,
    body: Option<Value>,
    status: ::http::StatusCode,
) -> (::http::Method, Option<Value>) {
    let downgrades = matches!(status.as_u16(), 301..=303) && *method != ::http::Method::GET;
    if downgrades {
        (::http::Method::GET, None)
    } else {
        (method.clone(), body)
    }
}

fn header_map(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<::http::HeaderMap, CallError> {
    let mut out = ::http::HeaderMap::new();
    for (name, value) in headers {
        let name =
            ::http::HeaderName::try_from(name.as_str()).map_err(|_| CallError::InvalidHeader)?;
        let value = ::http::HeaderValue::from_str(value).map_err(|_| CallError::InvalidHeader)?;
        out.insert(name, value);
    }
    Ok(out)
}

fn map_transport_err(err: reqwest::Error) -> CallError {
    if err.is_timeout() {
        return CallError::Timeout;
    }
    CallError::Transport {
        message: redact_message(&error_chain(&err)),
    }
}

/// `reqwest::Error`'s own `Display` is just the top-level "error sending request for url (...)"
/// — the actually useful detail (e.g. `http::resolver::DnsError`'s message, for a request that
/// never got past DNS resolution) lives in `.source()`. Walking the chain is what makes a guard
/// rejection distinguishable from a real network failure in a returned `CallError`.
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(next) = source {
        message.push_str(": ");
        message.push_str(&next.to_string());
        source = next.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_redirected_by_303_stays_get_with_no_body_change() {
        let (method, body) = downgrade(&::http::Method::GET, None, ::http::StatusCode::SEE_OTHER);
        assert_eq!(method, ::http::Method::GET);
        assert_eq!(body, None);
    }

    #[test]
    fn post_redirected_by_302_downgrades_to_get_and_drops_body() {
        let (method, body) = downgrade(
            &::http::Method::POST,
            Some(serde_json::json!({"a": 1})),
            ::http::StatusCode::FOUND,
        );
        assert_eq!(method, ::http::Method::GET);
        assert_eq!(body, None);
    }

    #[test]
    fn post_redirected_by_307_preserves_method_and_body() {
        let original = Some(serde_json::json!({"a": 1}));
        let (method, body) = downgrade(
            &::http::Method::POST,
            original.clone(),
            ::http::StatusCode::TEMPORARY_REDIRECT,
        );
        assert_eq!(method, ::http::Method::POST);
        assert_eq!(body, original);
    }

    #[test]
    fn is_redirect_recognises_exactly_the_five_relevant_codes() {
        for code in [301, 302, 303, 307, 308] {
            assert!(is_redirect(
                ::http::StatusCode::from_u16(code).expect("valid")
            ));
        }
        for code in [200, 204, 304, 404, 500] {
            assert!(!is_redirect(
                ::http::StatusCode::from_u16(code).expect("valid")
            ));
        }
    }
}
