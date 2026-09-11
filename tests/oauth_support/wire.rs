//! Low-level HTTP-transport helpers shared by `tests/oauth.rs`, `tests/oauth_flow.rs` and
//! `tests/oauth_refresh.rs` — split out (via `#[path]`, since each `tests/*.rs` compiles as
//! its own crate) purely to keep every one of those files under the workspace's 400-line cap
//! without triplicating this boilerplate. Deliberately has no dependency on `common`/
//! `fixture`, so `tests/oauth_refresh.rs` (which needs neither) can include it too.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

/// PKCE S256 pair: a random-looking verifier and its matching challenge.
pub fn pkce_pair() -> (String, String) {
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Builds an `application/x-www-form-urlencoded`/query string from `pairs`, percent-encoding
/// each value.
pub fn qs(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{k}={}",
                percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes a single value for inclusion in a query string.
pub fn urlenc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Undoes [`qs`]'s percent-encoding — needed for a value (a `code`) pulled back out of a
/// `Location` header before it's re-encoded into the *next* request, or it would be encoded
/// twice (a literal `%` surviving into the second encoding pass corrupts the value on roughly
/// a third of runs, whenever the token happens to contain a `-` or `_`).
pub fn urldecode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .expect("valid utf8")
        .into_owned()
}

pub async fn send(router: &Router, req: Request<Body>) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let response = router.clone().oneshot(req).await.expect("router call");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading body")
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

pub fn location(headers: &http::HeaderMap) -> String {
    headers
        .get(header::LOCATION)
        .expect("a Location header")
        .to_str()
        .expect("ascii Location")
        .to_owned()
}

pub async fn get(
    router: &Router,
    uri: &str,
    cookie: Option<&str>,
) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    send(router, builder.body(Body::empty()).expect("valid request")).await
}

pub async fn post_json(
    router: &Router,
    uri: &str,
    body: Vec<u8>,
    bearer: Option<&str>,
) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    send(
        router,
        builder.body(Body::from(body)).expect("valid request"),
    )
    .await
}

pub async fn post_form(
    router: &Router,
    uri: &str,
    body: &str,
    cookie: Option<&str>,
) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    send(
        router,
        builder
            .body(Body::from(body.to_owned()))
            .expect("valid request"),
    )
    .await
}
