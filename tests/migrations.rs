//! Migration round-trip: `up` creates every table the schema section of the skeleton
//! plan lists, and `down` actually reverses `up` — not just "doesn't error", but leaves
//! zero of those tables behind, so a second `up` recreates them from a clean slate.
//!
//! Both tests return early when `TEST_DATABASE_URL` is unset, so `cargo test` stays green
//! on a machine with no Postgres (see `tests/common/mod.rs`).

mod common;

use anyhow::{Context, Result};
use api2mcp::migration::Migrator;
use common::ScratchDb;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use sea_orm_migration::MigratorTrait;

/// Every table the plan's "Database schema" section lists, across all seven migrations.
const EXPECTED_TABLES: &[&str] = &[
    "users",
    "sessions",
    "service_tokens",
    "meta",
    "oauth_clients",
    "oauth_codes",
    "oauth_tokens",
    "oauth_consents",
    "oauth_consent_requests",
    "services",
    "auth_providers",
    "api_calls",
    "api_call_params",
    "scripts",
    "script_params",
    "script_api_calls",
    "tags",
    "api_call_tags",
    "script_tags",
    "endpoints",
    "endpoint_aliases",
    "endpoint_auth_providers",
    "runs",
    "run_calls",
];

#[tokio::test]
async fn up_creates_every_expected_table() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };

    // Exercises the same locked path the server takes at startup, not just
    // `Migrator::up` directly.
    db.migrate_up().await?;

    for table in EXPECTED_TABLES {
        assert!(
            table_exists(&db.conn, table).await?,
            "table {table:?} missing after `migrate up`"
        );
    }

    db.teardown().await
}

#[tokio::test]
async fn up_down_up_round_trips() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };

    Migrator::up(&db.conn, None).await.context("first `up`")?;
    for table in EXPECTED_TABLES {
        assert!(
            table_exists(&db.conn, table).await?,
            "table {table:?} missing after the first `up`"
        );
    }

    Migrator::down(&db.conn, None)
        .await
        .context("`down` (all migrations)")?;
    for table in EXPECTED_TABLES {
        assert!(
            !table_exists(&db.conn, table).await?,
            "table {table:?} survived a full `down` — down() does not reverse up()"
        );
    }

    Migrator::up(&db.conn, None).await.context("second `up`")?;
    for table in EXPECTED_TABLES {
        assert!(
            table_exists(&db.conn, table).await?,
            "table {table:?} missing after re-applying `up`"
        );
    }

    db.teardown().await
}

async fn table_exists(conn: &impl ConnectionTrait, table: &str) -> Result<bool> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS (\
                SELECT 1 FROM information_schema.tables \
                WHERE table_schema = 'public' AND table_name = $1\
            ) AS present",
            [table.into()],
        ))
        .await
        .with_context(|| format!("checking whether {table:?} exists"))?
        .context("EXISTS query returned no row")?;
    row.try_get::<bool>("", "present")
        .with_context(|| format!("reading the EXISTS result for {table:?}"))
}
