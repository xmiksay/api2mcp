//! `api2mcp serve` — connects, migrates under the advisory lock (which also seeds the first
//! user — see `migration::m0007_seed_first_user`, idempotent and a no-op once one exists),
//! builds [`AppState`], starts the background retention sweep (`server::retention`), binds,
//! and serves until ctrl-c.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::config::Config;
use crate::db;
use crate::http::{HickoryDns, SsrfPolicy, UpstreamPool};
use crate::resolve::PlanCache;
use crate::server::{AppState, build_router, retention};

pub async fn run() -> Result<()> {
    let cfg = Config::from_env()?;
    cfg.log_summary();

    let conn = db::connect(&cfg.database_url).await?;
    db::run_migrations_locked(&conn)
        .await
        .context("running startup migrations")?;

    let cfg = Arc::new(cfg);
    let upstream = Arc::new(UpstreamPool::new(
        Arc::new(HickoryDns::new()),
        SsrfPolicy {
            allow_loopback: cfg.allow_loopback_upstream,
        },
    ));
    let plans = Arc::new(PlanCache::new());
    let state = AppState::new(conn.clone(), cfg.clone(), upstream, plans);
    let retention_task = retention::spawn(conn, cfg.clone());

    let router = build_router(state);
    let listener = TcpListener::bind(cfg.bind_addr())
        .await
        .with_context(|| format!("binding {}", cfg.bind_addr()))?;

    tracing::info!(bind = %cfg.bind_addr(), "api2mcp listening");
    // Printed rather than logged: it is the next thing someone needs, not a diagnostic. Without
    // a header the client authenticates through OAuth — the 401 advertises the authorization
    // server — which is why no token appears here; `api2mcp token mint` prints the header form.
    // Two lines, not one: `/mcp` (the control plane) and `/mcp/{slug}` (a curated endpoint) are
    // separate surfaces for separate agents now (see `server::mcp`'s module doc) — an operator
    // setting up Claude Code needs to know both exist and pick the one they mean.
    println!(
        "claude mcp add --transport http api2mcp-factory {}/mcp   # define services/api_calls/scripts/endpoints",
        cfg.base_url
    );
    println!(
        "claude mcp add --transport http api2mcp {}/mcp/<slug>   # use a curated endpoint's tools",
        cfg.base_url
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;

    // The sweep loop never returns on its own (it only exits via cancellation), so it must be
    // stopped explicitly here rather than left running past the server it belongs to.
    retention_task.abort();

    Ok(())
}

/// Waits for ctrl-c so `axum::serve` shuts down gracefully (finishing in-flight requests)
/// instead of being killed mid-response.
async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to install ctrl-c handler; shutting down anyway");
    }
}
