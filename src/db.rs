//! Database connection and migration startup.

use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::migration::Migrator;

/// Arbitrary but stable key for the migration advisory lock. SeaORM's `Migrator::up` takes no
/// lock of its own, so two replicas starting together would otherwise race and can corrupt the
/// schema. Any process running migrations must use this exact key.
pub const MIGRATION_LOCK_KEY: i64 = 0x0A2C_0DE0_0001;

pub async fn connect(database_url: &str) -> Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(database_url.to_owned());
    opt.sqlx_logging_level(tracing::log::LevelFilter::Debug);
    Database::connect(opt).await.with_context(|| {
        format!(
            "connecting to {}",
            crate::config::redact_db_url(database_url)
        )
    })
}

/// Run all pending migrations under a session-scoped `pg_advisory_lock`, so two replicas
/// starting at once can't race `Migrator::up` and corrupt the schema. The lock key is a
/// hardcoded constant, never user input, so it is safe to interpolate directly rather than
/// bind as a query parameter.
///
/// The lock is released on every path, including when migrating fails — a session-scoped
/// advisory lock would eventually be freed when the connection drops anyway, but leaving a
/// long-lived pooled connection holding it would wedge every future migration attempt.
pub async fn run_migrations_locked(db: &DatabaseConnection) -> Result<()> {
    db.execute_unprepared(&format!("SELECT pg_advisory_lock({MIGRATION_LOCK_KEY})"))
        .await
        .context("acquiring migration advisory lock")?;

    let up_result = Migrator::up(db, None)
        .await
        .context("running database migrations");

    if let Err(e) = db
        .execute_unprepared(&format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY})"))
        .await
    {
        tracing::error!(error = %e, "failed to release migration advisory lock");
    }

    up_result
}
