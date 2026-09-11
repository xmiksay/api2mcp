//! Shared harness for the MCP data-plane integration tests (`tests/mcp.rs`,
//! `tests/mcp_tool_calls.rs`) — split out (via `#[path]`, since each `tests/*.rs` compiles as
//! its own crate) purely to keep both files under the workspace's 400-line cap. Each test
//! imports `examples/demo.pack.yaml` (patched to point at a freshly started
//! `tests/fixture::Fixture`, exactly like `tests/pack_roundtrip.rs`'s
//! `demo_pack_resolves_and_runs_against_the_fixture`) into its own scratch database, mints a
//! service token, and drives the resulting endpoint over real HTTP-shaped requests.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use api2mcp::config::Config;
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::PlanCache;
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, Stores};

use crate::common::ScratchDb;
use crate::fixture::harness::loopback_pool;
use crate::fixture::{Behavior, Fixture};

/// Everything a test needs: the scratch db (kept alive so `stores()`-backed assertions after the
/// HTTP round trip can still query it, and so `teardown` can run at the end), the fixture
/// upstream, the built router, and a plaintext service token.
pub struct Harness {
    pub db: ScratchDb,
    pub stores: Stores,
    pub router: Router,
    pub token: String,
}

pub async fn setup() -> Result<Option<Harness>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "mcp-test@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;
    let minted = stores
        .service_token()
        .mint(user.id, "mcp test token".to_owned(), None)
        .await?;

    let fixture = Fixture::start().await;
    fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );
    fixture.set(
        "/items",
        Behavior::Json(json!({"items": [{"id": "1", "title": "One"}]})),
    );

    let text = std::fs::read_to_string("examples/demo.pack.yaml")?;
    let mut demo: Pack = serde_norway::from_str(&text)?;
    let svc = demo
        .services
        .get_mut("demo-api")
        .expect("demo-api in the pack");
    svc.base_url = fixture.base_url().to_string();
    svc.origin_allowlist = BTreeSet::from([fixture.base_url().to_string()]);
    pack::validate(&demo).expect("patched demo pack is still valid");
    pack::import(&stores, &demo, false).await?;

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("demo".to_owned()),
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
        token: minted.plaintext,
    }))
}

/// Sends a raw body to `path`, optionally bearer-authenticated, and returns the status, response
/// headers, and the body parsed as JSON (`Value::Null` for an empty body — the
/// `notifications/initialized` 202 case).
pub async fn post_raw(
    router: &Router,
    token: Option<&str>,
    path: &str,
    body: &[u8],
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let request = builder
        .body(Body::from(body.to_vec()))
        .expect("valid request");
    let response = router.clone().oneshot(request).await.expect("router call");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading response body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("valid json response body")
    };
    (status, headers, value)
}

pub async fn rpc(router: &Router, token: &str, path: &str, body: Value) -> Value {
    let (status, _headers, value) = post_raw(
        router,
        Some(token),
        path,
        &serde_json::to_vec(&body).expect("serializing request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected status: {value:?}");
    value
}

pub fn req(id: i64, method: &str, params: Option<Value>) -> Value {
    let mut obj = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let Some(p) = params {
        obj["params"] = p;
    }
    obj
}
