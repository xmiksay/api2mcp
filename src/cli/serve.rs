//! `api2mcp serve` — connects, migrates under the advisory lock (which also seeds the first
//! user — see `migration::m0007_seed_first_user`, idempotent and a no-op once one exists),
//! builds [`AppState`], binds, and serves until ctrl-c.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::config::Config;
use crate::db;
use crate::http::{HickoryDns, SsrfPolicy, UpstreamPool};
use crate::resolve::PlanCache;
use crate::server::{AppState, build_router};

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
    let state = AppState::new(conn, cfg.clone(), upstream, plans);

    let router = build_router(state);
    let listener = TcpListener::bind(cfg.bind_addr())
        .await
        .with_context(|| format!("binding {}", cfg.bind_addr()))?;

    tracing::info!(bind = %cfg.bind_addr(), "api2mcp listening");
    // Printed rather than logged: it is the next thing someone needs, not a diagnostic. Without
    // a header the client authenticates through OAuth — the 401 advertises the authorization
    // server — which is why no token appears here; `api2mcp token mint` prints the header form.
    println!(
        "claude mcp add --transport http api2mcp {}/mcp/{}",
        cfg.base_url, cfg.default_endpoint
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;

    Ok(())
}

/// Waits for ctrl-c so `axum::serve` shuts down gracefully (finishing in-flight requests)
/// instead of being killed mid-response.
async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to install ctrl-c handler; shutting down anyway");
    }
}
