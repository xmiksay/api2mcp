//! Shared setup for `tests/oauth.rs` and `tests/oauth_flow.rs` — split out (via `#[path]`) to
//! keep both under the 400-line cap. References `crate::common`/`crate::fixture`, so a parent
//! file including this one must declare `mod common;`/`mod fixture;` itself first (each
//! `tests/*.rs` compiles as its own crate — see `oauth_wire.rs`'s doc for why the
//! dependency-free helpers live separately from this one).

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use serde_json::json;

use api2mcp::config::Config;
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::PlanCache;
use api2mcp::server::auth::SESSION_COOKIE_NAME;
use api2mcp::server::{self, AppState};
use api2mcp::store::{NewUser, Stores};

use crate::common::ScratchDb;
use crate::fixture::harness::loopback_pool;
use crate::fixture::{Behavior, Fixture};

pub const REDIRECT_URI: &str = "http://client.example/callback";

pub struct Harness {
    pub db: ScratchDb,
    pub stores: Stores,
    pub router: Router,
    pub cfg: Config,
    /// The patched, validated demo pack — not yet imported for anyone. Ownership is per-user
    /// now, so importing it happens in [`login_cookie`], under whichever user is signing in for
    /// that test, rather than here under some owner no login could ever match.
    pub demo: Pack,
}

/// A scratch DB pointed at a running fixture (so a round trip can prove a token minted here is
/// actually usable on `/mcp`, exactly like `tests/mcp.rs`) and a router built from
/// `oauth::router()` merged with `mcp::router()`. The demo pack is parsed and patched to the
/// fixture's own address here, but not imported — see [`Harness::demo`]'s own doc.
pub async fn setup() -> Result<Option<Harness>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let fixture = Fixture::start().await;
    fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
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
    // Leak the fixture so its listener outlives `setup` for the duration of the test process
    // — mirrors `tests/mcp.rs`'s own fixture lifetime handling.
    std::mem::forget(fixture);

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_BASE_URL" => Some("http://test.local".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("demo".to_owned()),
        "A2M_ALLOW_LOOPBACK_UPSTREAM" => Some("1".to_owned()),
        _ => None,
    })?;
    let state = AppState::new(
        db.conn.clone(),
        Arc::new(cfg.clone()),
        Arc::new(loopback_pool()),
        Arc::new(PlanCache::new()),
    );
    let router = Router::new()
        .merge(server::oauth::router())
        .merge(server::mcp::router())
        .with_state(state);

    Ok(Some(Harness {
        db,
        stores,
        router,
        cfg,
        demo,
    }))
}

/// Registers a user + browser session directly through the store (setup, not under test),
/// imports `h.demo` owned by that new user (so an OAuth-authenticated call as them can actually
/// reach it — a pack imported under anyone else would be invisible to their session), and
/// returns the `Cookie` header value the OAuth handlers will read.
pub async fn login_cookie(h: &Harness, email: &str) -> Result<String> {
    let user = h
        .stores
        .user()
        .create(NewUser {
            email: email.to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;
    pack::import(&h.stores, &h.demo, false, user.id).await?;
    let token = api2mcp::server::auth::create_session(
        &h.db.conn,
        user.id,
        std::time::Duration::from_secs(3600),
    )
    .await?;
    Ok(format!("{SESSION_COOKIE_NAME}={token}"))
}
