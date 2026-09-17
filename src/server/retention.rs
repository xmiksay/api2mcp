//! Background data-retention sweep: deletes `runs` older than `Config::run_retention_days`
//! (its `run_calls` cascade automatically, see `RunStore::purge_expired`'s own doc) and purges
//! expired `sessions` via `SessionStore::purge_expired`. Started by `cli::serve::run` and
//! stopped with the server rather than left detached — see [`spawn`].

use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::store::{RunStore, SessionStore};

/// How often the sweep runs after its first pass. Retention is configured in whole days, so
/// anything finer than an hour buys no real precision — it would just be idle-DB round trips.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Rows deleted per `DELETE` in [`RunStore::purge_expired`]'s own loop. Small enough that one
/// batch never holds a transaction open long enough to contend with live traffic; large enough
/// that a long-neglected instance with a huge backlog still converges in a sane number of round
/// trips rather than one row at a time.
const RUN_PURGE_BATCH_SIZE: u64 = 5_000;

/// Spawns the sweep loop on the current Tokio runtime and returns its handle so the caller can
/// `.abort()` it during shutdown. `tokio::time::interval`'s first tick fires immediately, so the
/// first sweep runs shortly after startup rather than waiting a full [`SWEEP_INTERVAL`].
pub fn spawn(db: DatabaseConnection, cfg: Arc<Config>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            interval.tick().await;
            sweep_once(&db, cfg.run_retention_days).await;
        }
    })
}

/// One sweep pass. Exposed separately from [`spawn`] so tests can call it directly against a
/// scratch database instead of starting a server and waiting on a real timer.
///
/// A failure here must never end the task — this is background hygiene, not a request path, so
/// the right response to a transient database error is "log it and try again next tick", not
/// silently stopping retention for the rest of the process's life. Only a non-zero deletion is
/// logged: a "deleted 0" every hour is noise, and the count is the only evidence anyone gets
/// that this is working at all.
pub async fn sweep_once(db: &DatabaseConnection, run_retention_days: u32) {
    match RunStore::new(db.clone())
        .purge_expired(run_retention_days, RUN_PURGE_BATCH_SIZE)
        .await
    {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, "retention: purged expired runs"),
        Err(e) => tracing::error!(error = %e, "retention: failed to purge expired runs"),
    }

    match SessionStore::new(db.clone()).purge_expired().await {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, "retention: purged expired sessions"),
        Err(e) => tracing::error!(error = %e, "retention: failed to purge expired sessions"),
    }
}
