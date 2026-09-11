//! Shared harness for the admin JSON API integration tests (`tests/api.rs`, `tests/api_write.rs`,
//! `tests/api_test_run.rs`). Lives in a subdirectory (`api_support/mod.rs`, not `api_support.rs`)
//! purely so cargo's `tests/*.rs` auto-discovery doesn't try to run it as its own test binary —
//! the same trick `tests/common/` and `tests/fixture/` already use.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use api2mcp::config::Config;
use api2mcp::resolve::PlanCache;
use api2mcp::server::auth::{SESSION_COOKIE_NAME, create_session};
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, Stores};

use super::common::ScratchDb;
use super::fixture::harness::loopback_pool;

/// Everything a test needs: the scratch db (kept alive so post-request store assertions and
/// `teardown` both still work), the built router, and three ways in — an admin session cookie, an
/// `mcp`-scoped service token, and an `admin`-scoped one (I5's sharpest edge: even this must be
/// refused on a write route).
pub struct Harness {
    pub db: ScratchDb,
    pub stores: Stores,
    pub router: Router,
    pub admin_cookie: String,
    pub mcp_token: String,
    pub admin_scoped_token: String,
}

pub async fn setup() -> Result<Option<Harness>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let admin = stores
        .user()
        .create(NewUser {
            email: "api-test-admin@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
            is_admin: true,
        })
        .await?;
    let cookie_value = create_session(&db.conn, admin.id, Duration::from_secs(3600)).await?;
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={cookie_value}");

    let token_owner = stores
        .user()
        .create(NewUser {
            email: "api-test-token-owner@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
            is_admin: false,
        })
        .await?;
    let mcp_minted = stores
        .service_token()
        .mint(
            token_owner.id,
            "api test mcp token".to_owned(),
            vec!["mcp".to_owned()],
            None,
        )
        .await?;
    let admin_minted = stores
        .service_token()
        .mint(
            token_owner.id,
            "api test admin-scoped token".to_owned(),
            vec!["admin".to_owned()],
            None,
        )
        .await?;

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("default".to_owned()),
        "A2M_ALLOW_LOOPBACK_UPSTREAM" => Some("1".to_owned()),
        _ => None,
    })?;
    let state = AppState::new(
        db.conn.clone(),
        Arc::new(cfg),
        Arc::new(loopback_pool()),
        Arc::new(PlanCache::new()),
    );
    let router = build_router(state);

    Ok(Some(Harness {
        db,
        stores,
        router,
        admin_cookie,
        mcp_token: mcp_minted.plaintext,
        admin_scoped_token: admin_minted.plaintext,
    }))
}

async fn request(
    router: &Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let bytes = match body {
        Some(v) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            serde_json::to_vec(v).expect("serializing request body")
        }
        None => Vec::new(),
    };
    let request = builder
        .body(Body::from(bytes))
        .expect("building a valid request");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router call succeeds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading the response body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        // The `Caller` extractor's own rejection (`server::identity`) is a plain
        // `(StatusCode, &'static str)`, not JSON — every route this module actually reaches
        // returns JSON, but a request that never gets that far (no session cookie, a bad one)
        // doesn't. Falling back to a string keeps this helper usable for both.
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

/// A request authenticated as the admin session — the only caller every route in this module
/// accepts.
pub async fn admin(
    h: &Harness,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    request(
        &h.router,
        method,
        path,
        Some(&h.admin_cookie),
        None,
        body.as_ref(),
    )
    .await
}

/// A request authenticated as a bearer service token — used only to prove one never reaches a
/// route in this module, whatever its scope.
pub async fn bearer(
    h: &Harness,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    request(&h.router, method, path, None, Some(token), body.as_ref()).await
}

/// A request with no credential at all.
pub async fn anonymous(h: &Harness, method: Method, path: &str) -> (StatusCode, Value) {
    request(&h.router, method, path, None, None, None).await
}
