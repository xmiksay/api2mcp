//! I7 end to end through `runtime::fanout`/`runtime::partial`: the same batch of upstream calls,
//! run repeatedly and under several different completion orderings, must produce byte-identical
//! output every time — completion order is never observable, only input order is.

mod fixture;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use api2mcp::http::{SsrfPolicy, UrlTemplate};
use api2mcp::model::{Access, ApiCall, Budgets, Origin, Pagination};
use api2mcp::resolve::EndpointPlan;
use api2mcp::resolve::plan::{PlannedApiCall, PlannedTool, ToolTarget};
use api2mcp::runtime::budget::BudgetMeter;
use api2mcp::runtime::dispatch::{AuthProviders, DispatchContext};
use api2mcp::runtime::fanout::ConcurrencyLimits;
use api2mcp::runtime::partial::{ItemOutcome, run_batch};

use fixture::harness::{loopback_pool, service_for, slug};
use fixture::{Behavior, Fixture};

const N: usize = 10;

fn plan_with_n_calls(fixture: &Fixture) -> EndpointPlan {
    let service = service_for(fixture);
    let mut calls = BTreeMap::new();
    let mut tools = Vec::new();
    for i in 0..N {
        let name = format!("call-{i}");
        let call = ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: slug(&name),
            service_slug: service.slug.clone(),
            method: ::http::Method::GET,
            path_template: format!("/{name}"),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: Access::Read,
            idempotent: true,
            projection: None,
            pagination: Pagination::None,
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
            name: name.clone(),
            input_schema: serde_json::json!({}),
            target: ToolTarget::ApiCall(call.slug.clone()),
            budgets: Budgets::default(),
        });
        calls.insert(call.slug.clone(), planned);
    }
    EndpointPlan {
        owner_id: uuid::Uuid::nil(),
        slug: slug("ep-determinism"),
        write_ceiling: Access::Read,
        instructions: None,
        tools,
        calls,
        scripts: BTreeMap::new(),
        callable_by: BTreeMap::new(),
        origins: BTreeSet::from([Origin::of(&service.base_url).expect("valid origin")]),
        budgets: Budgets::default(),
        digest: "digest".to_owned(),
    }
}

/// Registers `/call-0`..`/call-{N-1}`, each with its own distinguishable body and its own
/// artificial response delay — the caller controls completion order entirely through `delays_ms`.
fn set_behaviors(fixture: &Fixture, delays_ms: &[u64; N]) {
    for (i, delay_ms) in delays_ms.iter().enumerate() {
        let body = serde_json::json!({"index": i, "payload": format!("value-{i}")});
        fixture.set(
            &format!("/call-{i}"),
            Behavior::SlowlorisTrickle {
                chunk_bytes: serde_json::to_vec(&body).expect("serializable"),
                delay: Duration::from_millis(*delay_ms),
                chunks: 1,
            },
        );
    }
}

/// Runs the full N-item batch once and returns the canonical serialized form of its results, in
/// input order — the thing every assertion in this file compares.
async fn run_once(plan: &EndpointPlan) -> Vec<u8> {
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
    let limits = ConcurrencyLimits::build(plan, None);
    let meter = BudgetMeter::new(Budgets::default());

    let items: Vec<(String, serde_json::Value)> = (0..N)
        .map(|i| (format!("call-{i}"), serde_json::json!({})))
        .collect();
    let outcome = run_batch(plan, &ctx, &limits, &meter, None, items).await;

    let values: Vec<serde_json::Value> = outcome
        .entries
        .iter()
        .map(|e| match &e.outcome {
            ItemOutcome::Ok(o) => o.value.clone(),
            other => panic!("determinism test expects every item to succeed, got {other:?}"),
        })
        .collect();
    serde_json::to_vec(&values).expect("serializable")
}

/// Reversed delays: item 0 answers last, item N-1 answers first — the exact opposite of input
/// order. `#[tokio::test(flavor = "multi_thread", worker_threads = 8)]` so the scheduler
/// genuinely interleaves these across real OS threads rather than a single-threaded runtime
/// happening to poll them in a convenient order.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn same_batch_run_fifty_times_is_byte_identical() {
    let f = Fixture::start().await;
    let delays: [u64; N] = std::array::from_fn(|i| ((N - i) as u64) * 4);
    set_behaviors(&f, &delays);
    let plan = plan_with_n_calls(&f);

    let baseline = run_once(&plan).await;
    for attempt in 0..50 {
        let bytes = run_once(&plan).await;
        assert_eq!(
            bytes, baseline,
            "run {attempt} diverged from the baseline output"
        );
    }
}

/// Several distinct completion orderings — ascending, fully reversed, "release #9 before #0"
/// specifically, and an arbitrary shuffle — must all yield the identical serialized result.
#[tokio::test]
async fn out_of_order_completion_under_several_permutations_yields_the_same_result() {
    let ascending: [u64; N] = std::array::from_fn(|i| (i as u64) * 4);
    let descending: [u64; N] = std::array::from_fn(|i| ((N - i) as u64) * 4);
    let release_last_before_first: [u64; N] = {
        let mut d = [20u64; N];
        d[0] = 60; // request #0 answers last
        d[N - 1] = 0; // request #9 answers first
        d
    };
    let shuffled: [u64; N] = [30, 5, 45, 10, 0, 50, 20, 40, 15, 35];

    let permutations = [ascending, descending, release_last_before_first, shuffled];

    let mut canonical: Option<Vec<u8>> = None;
    for (i, delays) in permutations.iter().enumerate() {
        let f = Fixture::start().await;
        set_behaviors(&f, delays);
        let plan = plan_with_n_calls(&f);
        let bytes = run_once(&plan).await;
        match &canonical {
            None => canonical = Some(bytes),
            Some(expected) => assert_eq!(
                &bytes, expected,
                "permutation {i} ({delays:?}) diverged from the canonical result"
            ),
        }
    }
}
