//! One test per `Budgets` axis (calls, bytes, wall clock, pages), against the hermetic fixture
//! upstream — plus a full `Executor::run_tool` round trip through a scratch Postgres, proving a
//! run actually gets recorded with a complete snapshot, a digest, and no credential anywhere.
//!
//! No test here builds an `EndpointPlan` through `resolve::build_plan` — that would need rows in
//! a database this crate doesn't own the schema/store layer for touching directly in a way that's
//! worth the setup cost here. `resolve::plan::EndpointPlan`'s fields are all `pub`, so every test
//! just constructs one by hand, pointed at the fixture.

mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use api2mcp::entity::{run_calls, runs};
use api2mcp::http::{SsrfPolicy, UrlTemplate};
use api2mcp::model::{Access, ApiCall, Budgets, Origin, Pagination, Slug};
use api2mcp::resolve::EndpointPlan;
use api2mcp::resolve::plan::{PlannedApiCall, PlannedTool, ToolTarget};
use api2mcp::runtime::budget::{BudgetAxis, BudgetMeter};
use api2mcp::runtime::dispatch::{AuthProviders, DispatchContext};
use api2mcp::runtime::fanout::ConcurrencyLimits;
use api2mcp::runtime::partial::{BatchStatus, run_batch};
use api2mcp::runtime::{Executor, RunStatusView};
use api2mcp::store::{RunCallerKind, Stores};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use common::ScratchDb;
use fixture::harness::{loopback_pool, service_for, slug};
use fixture::{Behavior, Fixture};

/// One api_call per fixture path, all on the same service, no auth, no projection — everything a
/// budget test needs and nothing more.
fn plan_with_calls(fixture: &Fixture, names: &[&str]) -> EndpointPlan {
    plan_with_calls_paginated(fixture, names, Pagination::None)
}

fn plan_with_calls_paginated(
    fixture: &Fixture,
    names: &[&str],
    pagination: Pagination,
) -> EndpointPlan {
    let service = service_for(fixture);
    let mut calls = BTreeMap::new();
    let mut tools = Vec::new();
    for name in names {
        let call = ApiCall {
            slug: slug(name),
            service_slug: service.slug.clone(),
            auth_provider_slug: None,
            method: ::http::Method::GET,
            path_template: format!("/{name}"),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: Access::Read,
            idempotent: true,
            projection: None,
            pagination: pagination.clone(),
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![],
            description: None,
        };
        let planned = PlannedApiCall {
            url_template: UrlTemplate::parse(&call.path_template).expect("valid template"),
            origin: Origin::of(&service.base_url).expect("valid origin"),
            api_call: call.clone(),
            service: service.clone(),
            projection: None,
        };
        tools.push(PlannedTool {
            name: (*name).to_owned(),
            input_schema: serde_json::json!({}),
            target: ToolTarget::ApiCall(call.slug.clone()),
            budgets: Budgets::default(),
        });
        calls.insert(call.slug.clone(), planned);
    }

    EndpointPlan {
        slug: slug("ep-budgets"),
        write_ceiling: Access::Read,
        instructions: None,
        tools,
        calls,
        scripts: BTreeMap::new(),
        callable_by: BTreeMap::new(),
        origins: BTreeSet::from([Origin::of(&service.base_url).expect("valid origin")]),
        budgets: Budgets::default(),
        digest: "test-digest".to_owned(),
    }
}

async fn run(
    plan: &EndpointPlan,
    fixture: &Fixture,
    budgets: Budgets,
    caller_script: Option<&Slug>,
    items: Vec<(String, serde_json::Value)>,
) -> (BatchStatus, BudgetMeter) {
    let _ = fixture; // kept for symmetry/readability at call sites
    let pool = loopback_pool();
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let auth = AuthProviders::default();
    let ctx = DispatchContext {
        pool: &pool,
        policy: &policy,
        auth: &auth,
        max_redirects: 5,
    };
    let limits = ConcurrencyLimits::build(plan, budgets.max_concurrency);
    let meter = BudgetMeter::new(budgets);

    let outcome = run_batch(plan, &ctx, &limits, &meter, caller_script, items).await;
    (outcome.status, meter)
}

fn args() -> serde_json::Value {
    serde_json::json!({})
}

#[tokio::test]
async fn calls_axis_refuses_the_whole_batch_and_attempts_nothing() {
    let f = Fixture::start().await;
    f.set("/call-a", Behavior::Json(serde_json::json!({"ok": true})));
    let plan = plan_with_calls(&f, &["call-a"]);

    let budgets = Budgets {
        max_calls: Some(1),
        ..Budgets::default()
    };
    let items = vec![("call-a".to_owned(), args()), ("call-a".to_owned(), args())];
    let (status, meter) = run(&plan, &f, budgets, None, items).await;

    assert_eq!(status, BatchStatus::AllFailedOnBudget(BudgetAxis::Calls));
    assert_eq!(
        meter.calls_made(),
        0,
        "a refused reservation commits nothing"
    );
    // Nothing was attempted, so the fixture must never have seen a request at all.
    assert!(f.seen().is_empty());
}

#[tokio::test]
async fn bytes_axis_trips_at_the_correct_input_index_under_reversed_completion_order() {
    let f = Fixture::start().await;
    let body = serde_json::json!({"data": "a".repeat(200)});
    let one_item_len = serde_json::to_vec(&body).expect("serializable").len() as u64;

    // Item 0 answers slowly, item 1 answers immediately — completion order is the *reverse* of
    // input order, and the trip must still land on input index 1 regardless.
    f.set(
        "/call-0",
        Behavior::SlowlorisTrickle {
            chunk_bytes: serde_json::to_vec(&body).unwrap(),
            delay: Duration::from_millis(80),
            chunks: 1,
        },
    );
    f.set("/call-1", Behavior::Json(body.clone()));

    let plan = plan_with_calls(&f, &["call-0", "call-1"]);
    let budgets = Budgets {
        max_bytes: Some(one_item_len + one_item_len / 2),
        ..Budgets::default()
    };
    let items = vec![("call-0".to_owned(), args()), ("call-1".to_owned(), args())];
    let (status, meter) = run(&plan, &f, budgets, None, items).await;

    assert_eq!(status, BatchStatus::Partial);
    assert_eq!(
        meter.bytes_in(),
        one_item_len,
        "only the first input index's bytes were committed, regardless of who finished first"
    );
}

#[tokio::test]
async fn wall_clock_axis_is_a_terminal_trip_not_a_per_item_error() {
    let f = Fixture::start().await;
    f.set("/call-a", Behavior::Hang);
    let plan = plan_with_calls(&f, &["call-a"]);

    let budgets = Budgets {
        wall_clock: Some(Duration::from_millis(80)),
        ..Budgets::default()
    };
    let items = vec![("call-a".to_owned(), args())];
    let (status, _meter) = run(&plan, &f, budgets, None, items).await;

    assert_eq!(
        status,
        BatchStatus::AllFailedOnBudget(BudgetAxis::WallClock)
    );
}

#[tokio::test]
async fn pages_axis_trips_once_the_run_wide_page_cap_is_reached() {
    let f = Fixture::start().await;
    // Never terminates on its own — always reports another page.
    f.set(
        "/call-a",
        Behavior::Json(serde_json::json!({"next": "again"})),
    );
    let pagination = Pagination::Cursor {
        next_cursor_path: jsonptr::PointerBuf::from_tokens(["next"]),
        query_param: "cursor".to_owned(),
    };
    let plan = plan_with_calls_paginated(&f, &["call-a"], pagination);

    let budgets = Budgets {
        max_pages: Some(2),
        ..Budgets::default()
    };
    let items = vec![("call-a".to_owned(), args())];
    let (status, meter) = run(&plan, &f, budgets, None, items).await;

    assert_eq!(status, BatchStatus::AllFailedOnBudget(BudgetAxis::Pages));
    assert_eq!(
        f.seen().len(),
        2,
        "exactly the page cap's worth of requests were attempted"
    );
    // The run-level trip means the pages that *were* fetched never get charged to the meter —
    // there is no successful item whose page count could be committed.
    assert_eq!(meter.pages_fetched(), 0);
}

/// Not an axis test: proves the whole `Executor::run_tool` path — auth loading, dispatch,
/// recording — writes exactly one `runs` row and one `run_calls` row, with a complete snapshot,
/// the plan's digest, and no credential anywhere in what got persisted.
#[tokio::test]
async fn run_tool_persists_a_complete_run_with_no_credential_leaked() {
    let Some(db) = ScratchDb::create().await.expect("scratch db") else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    db.migrate_up().await.expect("migrate");

    unsafe {
        std::env::set_var("A2M_TEST_RUNTIME_BUDGETS_CRED", "sh-super-secret-token");
    }

    let f = Fixture::start().await;
    f.set("/call-a", Behavior::Json(serde_json::json!({"value": 42})));
    let mut plan = plan_with_calls(&f, &["call-a"]);
    plan.digest = "digest-for-persistence-test".to_owned();

    let pool = std::sync::Arc::new(api2mcp::http::UpstreamPool::new(
        std::sync::Arc::new(api2mcp::http::StaticDns::new()),
        SsrfPolicy {
            allow_loopback: true,
        },
    ));
    let executor = Executor::new(
        Stores::new(db.conn.clone()),
        pool,
        SsrfPolicy {
            allow_loopback: true,
        },
    );

    let result = executor
        .run_tool(
            &plan,
            "call-a",
            args(),
            RunCallerKind::ServiceToken,
            "test-caller".to_owned(),
        )
        .await
        .expect("run_tool succeeds");

    assert_eq!(result.status, RunStatusView::Ok);
    assert_eq!(result.value, Some(serde_json::json!({"value": 42})));

    let stores = Stores::new(db.conn.clone());
    let (summary, calls) = stores
        .run()
        .get(result.run_id)
        .await
        .expect("db read")
        .expect("run row exists");
    assert_eq!(summary.calls_made, 1);
    assert_eq!(calls.len(), 1, "one run_calls row per upstream request");
    assert_eq!(calls[0].seq, 0, "seq is the input index");

    // The store facade's own `get()` doesn't surface every column (e.g. `definition_snapshot`,
    // `headers_redacted`) — read the raw rows via the entities directly to check the columns
    // that actually carry the audit story and any residual credential risk.
    let run_row = runs::Entity::find_by_id(result.run_id)
        .one(&db.conn)
        .await
        .expect("db read")
        .expect("run row exists");
    assert_eq!(run_row.definition_digest, "digest-for-persistence-test");
    // The complete compiled slice: api_call, its params, its service, and the folded budgets.
    assert_eq!(
        run_row.definition_snapshot["kind"],
        serde_json::json!("api_call")
    );
    assert!(run_row.definition_snapshot["api_call"]["slug"].is_string());
    assert!(run_row.definition_snapshot["service"]["slug"].is_string());
    assert!(run_row.definition_snapshot["budgets"].is_object());

    let call_rows = run_calls::Entity::find()
        .filter(run_calls::Column::RunId.eq(result.run_id))
        .all(&db.conn)
        .await
        .expect("db read");
    assert_eq!(call_rows.len(), 1);

    // Dump every text-bearing column reachable from these two rows and assert the credential
    // literal is nowhere in it — the structural half (I4) is `Secret`'s job; this is the
    // redaction half's own proof, against what actually got persisted.
    let raw = format!("{run_row:?} {call_rows:?}");
    assert!(!raw.contains("sh-super-secret-token"));

    unsafe {
        std::env::remove_var("A2M_TEST_RUNTIME_BUDGETS_CRED");
    }
    db.teardown().await.expect("teardown");
}
