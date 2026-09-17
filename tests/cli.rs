//! Integration tests for chunk C8's `api2mcp call`/`api2mcp script run`, driven through
//! `cli::call::execute`/`cli::script::execute` directly rather than `cli::call::run`/
//! `cli::script::run` (which read `Config::from_env()` and would fight over process-global
//! environment variables between concurrently running tests — see those modules' own doc
//! comments for why the split exists) or by spawning the binary. Skipped when
//! `TEST_DATABASE_URL` is unset, same convention as every other integration test in this suite
//! (see `tests/common/mod.rs`).
//!
//! Every test seeds a scratch database from `examples/demo.pack.yaml`, patched to point at a
//! freshly started fixture — the same accommodation `tests/pack_roundtrip.rs`/`tests/mcp.rs`
//! make for the same file.

mod common;
mod fixture;

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;

use api2mcp::cli::{call, script};
use api2mcp::http::SsrfPolicy;
use api2mcp::model::{ScriptDef, Slug, Tag};
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::{EndpointPlan, build_plan};
use api2mcp::script::RunScriptError;
use api2mcp::store::{RunCallerKind, RunStatus, Stores};
use uuid::Uuid;

use common::ScratchDb;
use fixture::harness::loopback_pool;
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn policy() -> SsrfPolicy {
    SsrfPolicy {
        allow_loopback: true,
    }
}

/// Seeds a scratch database from `examples/demo.pack.yaml` patched to point at `f`, and resolves
/// the "demo" endpoint. Returns `None` when `TEST_DATABASE_URL` is unset — every test using this
/// must check that case and return early, same convention as `tests/common::ScratchDb`.
async fn seed(f: &Fixture) -> Result<Option<(ScratchDb, Stores, EndpointPlan, Uuid)>> {
    let Some(db) = ScratchDb::create().await? else {
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let owner = db.create_user().await?;

    f.set(
        "/items/1",
        // `internal` is never in `get-item`'s projection (see examples/demo.pack.yaml) — its
        // presence here is what lets a test prove `--raw` actually shows something the
        // projected result doesn't.
        Behavior::Json(json!({"id": "1", "title": "One", "internal": "do-not-leak"})),
    );
    f.set(
        "/items",
        Behavior::Json(json!({"items": [{"id": "1", "title": "One"}]})),
    );

    let text = std::fs::read_to_string("examples/demo.pack.yaml")?;
    let mut demo: Pack = serde_norway::from_str(&text)?;
    let svc = demo
        .services
        .get_mut("demo-api")
        .expect("demo-api in the pack");
    svc.base_url = f.base_url().to_string();
    svc.origin_allowlist = BTreeSet::from([f.base_url().to_string()]);
    pack::validate(&demo).expect("patched demo pack is still valid");
    pack::import(&stores, &demo, false, owner).await?;

    let plan = build_plan(&stores, owner, &slug("demo")).await?;
    Ok(Some((db, stores, plan, owner)))
}

#[tokio::test]
async fn call_prints_the_projected_value_and_raw_shows_what_it_dropped() -> Result<()> {
    let f = Fixture::start().await;
    let Some((db, stores, plan, owner)) = seed(&f).await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let pool = loopback_pool();

    let outcome = call::execute(
        &stores,
        &pool,
        policy(),
        &plan,
        "get-item",
        json!({"id": "1"}),
    )
    .await?;

    assert_eq!(outcome.status, RunStatus::Ok);
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.projected, Some(json!({"id": "1", "title": "One"})));
    assert_eq!(
        outcome.raw,
        Some(json!({"id": "1", "title": "One", "internal": "do-not-leak"}))
    );
    assert_ne!(
        outcome.raw, outcome.projected,
        "--raw's whole point is showing what the projection dropped"
    );

    let (summary, _calls) = stores
        .run()
        .get(owner, outcome.run_id)
        .await?
        .expect("the run was recorded");
    assert_eq!(summary.summary.tool_name, "get-item");
    assert_eq!(summary.summary.status, RunStatus::Ok);
    // A CLI invocation is the trusted local operator, never a token/session/OAuth caller —
    // `cli::call::execute` must record `RunCallerKind::Cli`, not a hardcoded literal borrowed
    // from a different caller kind.
    assert_eq!(summary.caller_kind, RunCallerKind::Cli);

    db.teardown().await
}

#[tokio::test]
async fn call_on_an_unknown_slug_fails_cleanly_instead_of_panicking() -> Result<()> {
    let f = Fixture::start().await;
    let Some((db, stores, plan, _owner)) = seed(&f).await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let pool = loopback_pool();

    let err = call::execute(&stores, &pool, policy(), &plan, "does-not-exist", json!({}))
        .await
        .expect_err("an unknown tool name must be a clean error, not a panic");
    assert!(err.to_string().contains("does-not-exist"));

    db.teardown().await
}

#[tokio::test]
async fn call_on_a_script_tool_fails_cleanly_rather_than_dispatching_it() -> Result<()> {
    let f = Fixture::start().await;
    let Some((db, stores, plan, _owner)) = seed(&f).await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let pool = loopback_pool();

    // `item-summary` is the demo pack's script tool — `call` targets api_calls only and must
    // refuse it rather than silently misdispatch (`dispatch::resolve`'s own `NotAnApiCall`).
    let outcome = call::execute(&stores, &pool, policy(), &plan, "item-summary", json!({})).await?;
    assert_eq!(outcome.status, RunStatus::Error);
    assert!(outcome.error.unwrap().contains("script"));

    db.teardown().await
}

#[tokio::test]
async fn script_run_composes_two_api_calls_in_input_order_with_a_breakdown() -> Result<()> {
    let f = Fixture::start().await;
    let Some((db, stores, plan, owner)) = seed(&f).await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let pool = loopback_pool();

    let outcome =
        script::execute(&stores, &pool, policy(), &plan, "item-summary", json!({})).await?;

    assert_eq!(outcome.status, RunStatus::Ok);
    assert!(outcome.error.is_none());
    assert_eq!(
        outcome.value,
        Some(json!({"count": 1, "first": {"id": "1", "title": "One"}}))
    );
    // `item-summary`'s source calls `list` then `get` — input order, not completion order (I7).
    assert_eq!(outcome.calls.len(), 2);
    assert_eq!(outcome.calls[0].index, 0);
    assert_eq!(outcome.calls[0].name, "list");
    assert_eq!(outcome.calls[1].index, 1);
    assert_eq!(outcome.calls[1].name, "get");
    assert!(outcome.calls.iter().all(|e| e.outcome.is_ok()));

    let (summary, _calls) = stores
        .run()
        .get(owner, outcome.run_id)
        .await?
        .expect("the run was recorded");
    assert_eq!(summary.summary.tool_name, "item-summary");
    assert_eq!(summary.summary.calls_made, 2);

    db.teardown().await
}

#[tokio::test]
async fn script_run_surfaces_the_failing_line_and_a_non_ok_status() -> Result<()> {
    let f = Fixture::start().await;
    let Some((db, stores, _plan, owner)) = seed(&f).await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let pool = loopback_pool();

    // A hand-built script calling an alias it never declared — `dispatch::resolve` rejects it
    // (I1) before any HTTP happens, giving a deterministic runtime failure with a known line,
    // with no dependency on fixture behaviour. Tagged `demo` so the "demo" endpoint's
    // `has(demo)` tag_expr picks it up once the plan is rebuilt.
    let broken = ScriptDef {
        owner_id: owner,
        slug: slug("broken-script"),
        source: "let x = 1;\napi(\"not-declared\", #{});\n".to_owned(),
        params: vec![],
        callable: Default::default(),
        budgets: Default::default(),
        description: None,
    };
    stores
        .script()
        .create(&broken, &BTreeSet::from([Tag(slug("demo"))]))
        .await?;
    let plan = build_plan(&stores, owner, &slug("demo")).await?;

    let outcome =
        script::execute(&stores, &pool, policy(), &plan, "broken-script", json!({})).await?;

    assert_eq!(outcome.status, RunStatus::Error);
    assert_ne!(
        outcome.status,
        RunStatus::Ok,
        "cli::script::run must exit non-zero on exactly this status"
    );
    match outcome.error {
        Some(RunScriptError::Script(failure)) => {
            assert_eq!(failure.line, Some(2));
            assert!(
                failure.snippet.is_some(),
                "a debuggable failure needs a snippet"
            );
        }
        other => panic!("expected a script failure carrying a line number, got {other:?}"),
    }

    db.teardown().await
}
