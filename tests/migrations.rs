//! Migration round-trip: `up` creates every table the schema section of the skeleton
//! plan lists, and `down` actually reverses `up` — not just "doesn't error", but leaves
//! zero of those tables behind, so a second `up` recreates them from a clean slate.
//!
//! Both tests return early when `TEST_DATABASE_URL` is unset, so `cargo test` stays green
//! on a machine with no Postgres (see `tests/common/mod.rs`).

mod common;

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use api2mcp::migration::Migrator;
use api2mcp::model::{Access, ApiCall, Origin, Pagination, Service, Slug};
use api2mcp::store::{StoreError, Stores};
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

/// `api_calls.slug` is `UNIQUE` per owner (`ux_api_calls_owner_slug`), not per-service — a
/// bare slug is what `script_api_calls`, `endpoint_aliases.target_slug` and pack import/
/// export all reference, so two services *belonging to the same owner* defining the same
/// slug must be a constraint violation, not a runtime ambiguity `store::api_call` has to
/// resolve after the fact. (Two different owners defining the same slug is fine — that is
/// the whole point of per-owner uniqueness — so this test pins the narrower, still-true
/// half of the old "globally unique" claim.)
#[tokio::test]
async fn api_call_slug_is_unique_across_services_for_the_same_owner() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let owner_id = db.create_user().await?;

    let a = sample_service(owner_id, "dup-slug-svc-a");
    let b = sample_service(owner_id, "dup-slug-svc-b");
    stores.service().create(&a).await?;
    stores.service().create(&b).await?;

    stores
        .api_call()
        .create(
            &sample_api_call(owner_id, &a.slug, "shared"),
            &BTreeSet::new(),
        )
        .await?;
    let err = stores
        .api_call()
        .create(
            &sample_api_call(owner_id, &b.slug, "shared"),
            &BTreeSet::new(),
        )
        .await
        .expect_err("an owner-unique slug must reject a second service reusing it");
    assert!(matches!(err, StoreError::Db), "got {err:?} instead");

    db.teardown().await
}

fn sample_service(owner_id: uuid::Uuid, name: &str) -> Service {
    let base_url: url::Url = format!("https://{name}.example.com/").parse().unwrap();
    Service {
        owner_id,
        slug: name.parse().unwrap(),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
        default_headers: Default::default(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

fn sample_api_call(owner_id: uuid::Uuid, service_slug: &Slug, name: &str) -> ApiCall {
    ApiCall {
        owner_id,
        slug: name.parse().unwrap(),
        service_slug: service_slug.clone(),
        auth_provider_slug: None,
        method: http::Method::GET,
        path_template: "/things".to_owned(),
        query_fixed: Default::default(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: vec![],
        description: None,
    }
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
