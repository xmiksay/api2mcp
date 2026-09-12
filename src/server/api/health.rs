//! `GET /api/health` — version, migration level, DB connectivity, and endpoint count.
//! Session-only like every other route in this module (see `super`'s doc); this is not a
//! load-balancer probe.

use axum::Json;
use axum::extract::State;
use sea_orm_migration::MigratorTrait;
use serde::Serialize;

use crate::migration::Migrator;
use crate::server::state::AppState;
use crate::version;

use super::{ApiError, Caller};

#[derive(Debug, Serialize)]
pub struct HealthView {
    pub version: &'static str,
    pub commit: &'static str,
    pub db_connected: bool,
    pub migrations_applied: usize,
    pub migrations_total: usize,
    pub endpoint_count: usize,
}

pub async fn health(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<HealthView>, ApiError> {
    let db_connected = state.db.ping().await.is_ok();
    // A migration query only makes sense once connectivity is established — probing it on a
    // dead connection would just be a second, redundant way to observe the same failure.
    let migrations_applied = if db_connected {
        Migrator::get_applied_migrations(&state.db)
            .await
            .map(|rows| rows.len())
            .unwrap_or(0)
    } else {
        0
    };
    // Scoped to the caller's own endpoints, same as every other read in `server::api` — there is
    // no more a system-wide "every endpoint" count to report than there is a system-wide list.
    let endpoint_count = if db_connected {
        state
            .stores()
            .endpoint()
            .list_all(caller.id)
            .await
            .map(|rows| rows.len())
            .unwrap_or(0)
    } else {
        0
    };

    Ok(Json(HealthView {
        version: version::VERSION,
        commit: version::COMMIT,
        db_connected,
        migrations_applied,
        migrations_total: Migrator::migrations().len(),
        endpoint_count,
    }))
}
