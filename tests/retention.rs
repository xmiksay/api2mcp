//! `server::retention` — the background sweep that keeps `runs`/`run_calls`/`sessions` from
//! growing without bound. Everything here goes straight through `store::` (no HTTP layer, no
//! fixture server): retention doesn't care what a run's contents look like, only its age, and
//! `RunStore::create`/`SessionStore::create` are all the fixture this needs.
//!
//! Skipped when `TEST_DATABASE_URL` is unset, same as every other scratch-Postgres test — see
//! `tests/common/mod.rs`.

mod common;

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use api2mcp::entity::run_calls;
use api2mcp::model::Slug;
use api2mcp::server::retention;
use api2mcp::store::{
    NewRun, NewRunCall, RunCallerKind, RunFilter, RunStatus, RunTargetKind, SessionStore, Stores,
};

use common::ScratchDb;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn sample_run(owner_id: Uuid) -> NewRun {
    NewRun {
        owner_id,
        endpoint_slug: slug("ep"),
        tool_name: "thing".to_owned(),
        target_kind: RunTargetKind::ApiCall,
        target_slug: slug("thing"),
        caller_kind: RunCallerKind::Oauth,
        caller_id: owner_id.to_string(),
        request_id: Uuid::new_v4().to_string(),
        execution_start: Utc::now(),
        definition_snapshot: json!({}),
        definition_digest: "deadbeef".to_owned(),
        input_redacted: json!({}),
        output_redacted: None,
        status: RunStatus::Ok,
        errors: None,
        calls_made: 1,
        bytes_in: 0,
        pages_fetched: 0,
        budget_snapshot: None,
        timings: None,
    }
}

fn sample_call() -> NewRunCall {
    NewRunCall {
        seq: 0,
        api_call_slug: slug("thing"),
        service_slug: slug("svc"),
        method: http::Method::GET,
        url_redacted: "https://example.com/thing".to_owned(),
        headers_redacted: None,
        body_redacted: None,
        status_code: Some(200),
        response_bytes: Some(10),
        response_truncated: false,
        error: None,
        timings: None,
    }
}

/// `RunStore::create` always lands `created_at` on the DB's own `now()` (see `m0006_runs`'s
/// `timestamptz_now` default) — there's no `NewRun` field for it, deliberately, since nothing
/// outside a test should ever want to backdate a run. This is the one place that's the right
/// call, via the same raw-`Statement` mechanism the store itself uses for the actual purge.
async fn backdate(
    conn: &DatabaseConnection,
    run_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<()> {
    conn_execute(
        conn,
        "UPDATE runs SET created_at = $1 WHERE id = $2",
        [created_at.into(), run_id.into()],
    )
    .await
}

async fn conn_execute(
    conn: &DatabaseConnection,
    sql: &str,
    values: impl IntoIterator<Item = sea_orm::Value>,
) -> Result<()> {
    use sea_orm::ConnectionTrait;
    conn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await?;
    Ok(())
}

#[tokio::test]
async fn expired_runs_and_their_calls_are_deleted_newer_ones_survive() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        return Ok(());
    };
    db.migrate_up().await?;
    let owner_id = db.create_user().await?;
    let stores = Stores::new(db.conn.clone());

    let old_id = stores
        .run()
        .create(&sample_run(owner_id), &[sample_call()])
        .await?;
    backdate(&db.conn, old_id, Utc::now() - ChronoDuration::days(40)).await?;
    let new_id = stores
        .run()
        .create(&sample_run(owner_id), &[sample_call()])
        .await?;

    let deleted = stores.run().purge_expired(30, 1_000).await?;
    assert_eq!(deleted, 1);

    assert!(
        stores.run().get(owner_id, old_id).await?.is_none(),
        "the expired run must be gone"
    );
    assert!(
        stores.run().get(owner_id, new_id).await?.is_some(),
        "the recent run must survive"
    );
    assert_eq!(
        run_calls::Entity::find()
            .filter(run_calls::Column::RunId.eq(old_id))
            .count(&db.conn)
            .await?,
        0,
        "run_calls must cascade with its run"
    );
    assert_eq!(
        run_calls::Entity::find()
            .filter(run_calls::Column::RunId.eq(new_id))
            .count(&db.conn)
            .await?,
        1,
        "the surviving run keeps its call"
    );

    db.teardown().await
}

#[tokio::test]
async fn batching_terminates_and_deletes_every_eligible_row() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        return Ok(());
    };
    db.migrate_up().await?;
    let owner_id = db.create_user().await?;
    let stores = Stores::new(db.conn.clone());

    // More rows than the artificially small batch size below, so a single pass can't possibly
    // finish the job — this is the assertion that the loop in `RunStore::purge_expired` keeps
    // going until it actually converges, not just that it deletes *something*.
    const SEEDED: usize = 11;
    const BATCH_SIZE: u64 = 3;
    for _ in 0..SEEDED {
        let id = stores.run().create(&sample_run(owner_id), &[]).await?;
        backdate(&db.conn, id, Utc::now() - ChronoDuration::days(2)).await?;
    }

    let deleted = stores.run().purge_expired(1, BATCH_SIZE).await?;
    assert_eq!(deleted, SEEDED as u64);

    let remaining = stores
        .run()
        .list(
            owner_id,
            &RunFilter {
                endpoint_slug: None,
                status: None,
                limit: 100,
                offset: 0,
            },
        )
        .await?;
    assert!(remaining.is_empty(), "every eligible row must be gone");

    db.teardown().await
}

#[tokio::test]
async fn zero_retention_days_deletes_nothing_end_to_end() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        return Ok(());
    };
    db.migrate_up().await?;
    let owner_id = db.create_user().await?;
    let stores = Stores::new(db.conn.clone());

    let run_id = stores
        .run()
        .create(&sample_run(owner_id), &[sample_call()])
        .await?;
    // Absurdly old — if `run_retention_days == 0` were ever misread as "delete everything"
    // instead of "keep forever", this row would be the first one gone.
    backdate(&db.conn, run_id, Utc::now() - ChronoDuration::days(3650)).await?;

    // Through the actual wiring `cli::serve::run` uses, not just `RunStore::purge_expired`
    // directly — this is the "test the wiring end to end" case.
    retention::sweep_once(&db.conn, 0).await;

    assert!(
        stores.run().get(owner_id, run_id).await?.is_some(),
        "run_retention_days == 0 must keep every run, however old"
    );

    db.teardown().await
}

#[tokio::test]
async fn expired_sessions_are_purged_live_ones_are_not() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        return Ok(());
    };
    db.migrate_up().await?;
    let owner_id = db.create_user().await?;
    let sessions = SessionStore::new(db.conn.clone());

    sessions
        .create(
            "expired-token",
            owner_id,
            Utc::now() - ChronoDuration::hours(1),
        )
        .await?;
    sessions
        .create(
            "live-token",
            owner_id,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await?;

    let deleted = sessions.purge_expired().await?;
    assert_eq!(deleted, 1);

    assert!(
        sessions.resolve("expired-token").await?.is_none(),
        "the expired session is gone"
    );
    assert!(
        sessions.resolve("live-token").await?.is_some(),
        "the live session survives the sweep"
    );

    db.teardown().await
}
