//! Database connection and migration startup.

use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

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
