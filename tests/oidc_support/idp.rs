//! A minimal, real OIDC provider for `tests/oidc_login.rs` — a genuine `axum::serve` listener
//! (needed because `server::oidc` talks to it over `reqwest`, not `tower::Service::oneshot`),
//! implementing just enough of discovery/token/userinfo to exercise the relying-party client for
//! real: it actually recomputes the PKCE `S256` challenge from the `code_verifier` it receives
//! and rejects a mismatch, exactly like a real provider would.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const ACCESS_TOKEN: &str = "test-access-token";

#[derive(Default)]
struct Inner {
    /// Set right before the callback step, once the test has read `code_challenge` back out of
    /// the `/login/oidc/start` redirect — a real provider would have stashed this itself, keyed
    /// by the authorization code, at its own `/authorize` step.
    expected_code_challenge: Option<String>,
    sub: String,
    email: String,
    /// `None` omits the `email_verified` claim from `/userinfo` entirely (some providers never
    /// send it); `Some(v)` sends it as a real JSON boolean. Either way this is the mock's only
    /// way to control it — a real provider wouldn't let this crate set the claim itself.
    email_verified: Option<bool>,
    /// Every `Authorization` header value the `/token` endpoint has ever seen — lets a test
    /// assert `client_secret_basic` was actually used, and exactly what it carried.
    seen_token_auth_headers: Vec<String>,
}

#[derive(Clone)]
pub struct MockIdp {
    pub base_url: String,
    inner: Arc<Mutex<Inner>>,
}

impl MockIdp {
    pub async fn start() -> Self {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral loopback port");
        let addr: SocketAddr = listener.local_addr().expect("listener has a local address");
        let base_url = format!("http://{addr}");

        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .with_state((base_url.clone(), Arc::clone(&inner)));
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock idp run loop");
        });

        Self { base_url, inner }
    }

    /// Records the `code_challenge` the real `/token` check must match — the test reads this
    /// out of `GET /login/oidc/start`'s redirect URL first.
    pub fn expect_code_challenge(&self, challenge: &str) {
        self.inner.lock().expect("idp lock").expected_code_challenge = Some(challenge.to_owned());
    }

    /// Sets the identity `/userinfo` reports, with `email_verified: true` — the common case for
    /// every existing test, none of which cares about the claim path this field also gates.
    pub fn set_identity(&self, sub: &str, email: &str) {
        self.set_identity_verified(sub, email, Some(true));
    }

    /// As [`Self::set_identity`], but with explicit control over the `email_verified` claim:
    /// `Some(v)` sends it as `v`, `None` omits it from the `/userinfo` response entirely.
    pub fn set_identity_verified(&self, sub: &str, email: &str, email_verified: Option<bool>) {
        let mut inner = self.inner.lock().expect("idp lock");
        inner.sub = sub.to_owned();
        inner.email = email.to_owned();
        inner.email_verified = email_verified;
    }

    pub fn seen_token_auth_headers(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("idp lock")
            .seen_token_auth_headers
            .clone()
    }
}

async fn discovery(State((base_url, _)): State<(String, Arc<Mutex<Inner>>)>) -> Json<Value> {
    Json(json!({
        "authorization_endpoint": format!("{base_url}/authorize"),
        "token_endpoint": format!("{base_url}/token"),
        "userinfo_endpoint": format!("{base_url}/userinfo"),
    }))
}

async fn token(
    State((_, inner)): State<(String, Arc<Mutex<Inner>>)>,
    headers: HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Response {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    let expected_challenge = {
        let mut inner = inner.lock().expect("idp lock");
        inner.seen_token_auth_headers.push(auth_header);
        inner.expected_code_challenge.clone()
    };

    let Some(verifier) = params.get("code_verifier") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request"})),
        )
            .into_response();
    };
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if Some(&computed) != expected_challenge.as_ref() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }

    (StatusCode::OK, Json(json!({"access_token": ACCESS_TOKEN}))).into_response()
}

async fn userinfo(
    State((_, inner)): State<(String, Arc<Mutex<Inner>>)>,
    headers: HeaderMap,
) -> Response {
    let expected = format!("Bearer {ACCESS_TOKEN}");
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if presented != expected {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let inner = inner.lock().expect("idp lock");
    let mut body = json!({"sub": inner.sub, "email": inner.email});
    if let Some(verified) = inner.email_verified {
        body["email_verified"] = json!(verified);
    }
    (StatusCode::OK, Json(body)).into_response()
}
