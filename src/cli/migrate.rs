//! `api2mcp migrate up|down [steps]|status` — connects with [`Config::from_env`] and drives
//! [`crate::migration::Migrator`] directly; the server does the same `up` at startup via
//! [`crate::db::run_migrations_locked`].

use anyhow::{Context, Result};
use sea_orm_migration::MigratorTrait;

use crate::cli::MigrateAction;
use crate::config::Config;
use crate::db;
use crate::migration::Migrator;

pub async fn run(action: MigrateAction) -> Result<()> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;

    match action {
        MigrateAction::Up => {
            db::run_migrations_locked(&conn).await?;
            println!("migrations applied");
        }
        MigrateAction::Down { steps } => {
            // `Migrator::down`'s own default (steps = None) rolls back *every* applied
            // migration. That is a much bigger footgun for an interactive CLI than "one
            // step unless told otherwise", so an absent `steps` is rewritten here rather
            // than passed through.
            let steps = steps.or(Some(1));
            Migrator::down(&conn, steps)
                .await
                .context("rolling back migrations")?;
            println!("migrations rolled back");
        }
        MigrateAction::Status => {
            // Not `Migrator::status` — it logs through the `log` facade, which nothing in
            // this binary bridges into `tracing`, so it would print nothing at all here.
            let migrations = Migrator::get_migration_with_status(&conn)
                .await
                .context("checking migration status")?;
            for m in migrations {
                println!("{:<10} {}", m.status(), m.name());
            }
        }
    }

    Ok(())
}
