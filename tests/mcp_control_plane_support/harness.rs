//! Shared harness for the `/mcp` control-plane integration tests (`tests/mcp_control_plane.rs`,
//! `tests/mcp_control_plane_auth.rs`) — split out (via `#[path]`, since each `tests/*.rs`
//! compiles as its own crate) purely to keep every file under the workspace's 400-line cap, the
//! same convention `tests/mcp_support/harness.rs` already uses for the data plane.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use api2mcp::config::Config;
use api2mcp::resolve::PlanCache;
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, Stores};

use crate::common::ScratchDb;
use crate::fixture::Fixture;
use crate::fixture::harness::loopback_pool;

/// Everything a control-plane test needs: the scratch db, the fixture upstream, the router, and
/// two tokens — one minted `control_plane: true` (the normal "authoring agent" token) and one
/// without it (every token minted before this feature existed, and every one minted since
/// without opting in).
pub struct Cp {
    pub db: ScratchDb,
    pub stores: Stores,
    pub router: axum::Router,
    pub fixture: Fixture,
    pub owner_id: Uuid,
    pub control_token: String,
    pub plain_token: String,
}

pub async fn setup() -> Result<Option<Cp>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "control-plane@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;

    let control = stores
        .service_token()
        .mint(
            user.id,
            "authoring agent".to_owned(),
            None,
            BTreeSet::new(),
            true,
        )
        .await?;
    let plain = stores
        .service_token()
        .mint(
            user.id,
            "data-plane only".to_owned(),
            None,
            BTreeSet::new(),
            false,
        )
        .await?;

    let fixture = Fixture::start().await;

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
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

    Ok(Some(Cp {
        db,
        stores,
        router,
        fixture,
        owner_id: user.id,
        control_token: control.plaintext,
        plain_token: plain.plaintext,
    }))
}

pub fn service_body(slug: &str, base_url: &url::Url) -> Value {
    json!({
        "slug": slug,
        "base_url": base_url.to_string(),
        "origin_allowlist": [base_url.to_string().trim_end_matches('/')],
        "timeout_ms": 5000,
        "max_concurrency": 4,
        "max_response_bytes": 1_000_000
    })
}

pub fn api_call_body(slug: &str, service: &str, path: &str, tags: &[&str]) -> Value {
    json!({
        "slug": slug,
        "service": service,
        "method": "GET",
        "path_template": path,
        "access": "read",
        "tags": tags
    })
}

pub fn endpoint_body(slug: &str, tag_expr: &str) -> Value {
    json!({ "slug": slug, "tag_expr": tag_expr })
}

/// A `tools/call` JSON-RPC request — built inline rather than through
/// `mcp_support::harness::req` to keep this support module self-contained (no cross-support-
/// module path coupling between two independent `#[path]`-included harnesses).
pub fn call_tool(id: i64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

/// A tool result's decoded `content[0].text`, parsed as JSON — every control-plane tool's
/// success and business-failure text is a JSON document (see `control::support::api_error_outcome`
/// and `ok_value`).
pub fn tool_json(resp: &Value) -> Value {
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("expected text content in {resp:?}"));
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

pub fn is_error(resp: &Value) -> bool {
    resp["result"]["isError"] == json!(true)
}

pub async fn create_minimal_endpoint(stores: &Stores, owner_id: Uuid, slug: &str) -> Result<()> {
    use api2mcp::model::{Access, Budgets, EndpointDef, Tag, TagExpr};
    stores
        .endpoint()
        .create(&EndpointDef {
            owner_id,
            slug: slug.parse().unwrap(),
            tag_expr: TagExpr::Has(Tag("unused".parse().unwrap())),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: Default::default(),
            auth_providers: BTreeSet::new(),
        })
        .await?;
    Ok(())
}
