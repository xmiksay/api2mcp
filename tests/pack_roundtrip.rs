//! `src/pack/` integration tests, against real scratch Postgres instances (skipped when
//! `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`). The headline assertion is
//! [`pack_export_reimport_reproduces_identical_resolve_digest`]: export from one instance,
//! import into a completely separate one, and prove `resolve::build_plan` produces a
//! byte-identical `EndpointPlan::digest` — that single assertion is what proves a pack is
//! genuinely portable, not merely well-formed YAML.

mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use common::ScratchDb;

use api2mcp::http::{SsrfPolicy, UpstreamPool};
use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Service, Slug, Tag, TagExpr,
};
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::build_plan;
use api2mcp::store::Stores;
use fixture::harness::loopback_pool;
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn tag(s: &str) -> Tag {
    Tag(slug(s))
}

fn service(name: &str) -> Service {
    let base_url: url::Url = format!("https://{name}.example.com/").parse().unwrap();
    Service {
        slug: slug(name),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

/// Seeds one service, one tagged api_call, and one endpoint selecting it — the smallest
/// definition set that still gives `resolve::build_plan` something to compile.
async fn seed_minimal(stores: &Stores, suffix: &str) -> Result<Slug> {
    let svc = service(&format!("svc-{suffix}"));
    stores.service().create(&svc).await?;
    let call = ApiCall {
        slug: slug(&format!("call-{suffix}")),
        service_slug: svc.slug.clone(),
        auth_provider_slug: None,
        method: http::Method::GET,
        path_template: "/things".to_owned(),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: Vec::new(),
    };
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = EndpointDef {
        slug: slug(&format!("ep-{suffix}")),
        tag_expr: TagExpr::Has(tag("expose")),
        write_ceiling: Access::Read,
        budgets: Budgets::default(),
        instructions: None,
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    };
    stores.endpoint().create(&ep).await?;
    Ok(ep.slug)
}

/// The highest-value test: seed definitions in one database, export, import into a *separate*
/// scratch database (standing in for "wipe and reimport" — a fresh instance is a stronger proof
/// of portability than the same database emptied), resolve in both, and assert an identical
/// `EndpointPlan::digest`.
#[tokio::test]
async fn pack_export_reimport_reproduces_identical_resolve_digest() -> Result<()> {
    let Some(source) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    source.migrate_up().await?;
    let source_stores = Stores::new(source.conn.clone());
    let ep_slug = seed_minimal(&source_stores, "digest").await?;

    let exported = pack::export_endpoint(&source_stores, &ep_slug).await?;
    pack::validate(&exported).expect("exported pack is always valid");

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());
    pack::import(&target_stores, &exported, false).await?;

    let plan_source = build_plan(&source_stores, &ep_slug).await?;
    let plan_target = build_plan(&target_stores, &ep_slug).await?;
    assert_eq!(plan_source.digest, plan_target.digest);
    assert_eq!(plan_source.origins, plan_target.origins);

    source.teardown().await?;
    target.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn dry_run_writes_nothing() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let ep_slug = seed_minimal(&stores, "dryrun-source").await?;
    let exported = pack::export_endpoint(&stores, &ep_slug).await?;

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());

    let report = pack::import(&target_stores, &exported, true).await?;
    assert!(
        !report.is_idempotent_no_op(),
        "a dry run into an empty db reports creates"
    );
    assert!(target_stores.endpoint().get(&ep_slug).await?.is_none());
    for slug_str in exported.services.keys() {
        assert!(
            target_stores
                .service()
                .get_by_slug(&slug(slug_str))
                .await?
                .is_none()
        );
    }

    db.teardown().await?;
    target.teardown().await?;
    Ok(())
}

#[tokio::test]
async fn import_is_idempotent_in_state_and_digest() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let ep_slug = seed_minimal(&stores, "idem-source").await?;
    let exported = pack::export_endpoint(&stores, &ep_slug).await?;

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());

    let first = pack::import(&target_stores, &exported, false).await?;
    assert!(!first.is_idempotent_no_op());
    let digest_after_first = build_plan(&target_stores, &ep_slug).await?.digest;

    let second = pack::import(&target_stores, &exported, false).await?;
    assert!(
        second.is_idempotent_no_op(),
        "re-importing an unchanged pack must be a no-op"
    );
    let digest_after_second = build_plan(&target_stores, &ep_slug).await?.digest;
    assert_eq!(digest_after_first, digest_after_second);

    db.teardown().await?;
    target.teardown().await?;
    Ok(())
}

#[test]
fn validation_reports_multiple_independent_failures_at_once() {
    let pack = Pack {
        version: 999, // failure #1: unsupported version
        services: BTreeMap::new(),
        auth_providers: BTreeMap::new(),
        api_calls: BTreeMap::new(),
        scripts: BTreeMap::new(),
        endpoints: BTreeMap::from([(
            "ep-bad".to_owned(),
            api2mcp::pack::PackEndpoint {
                tag_expr: "has(".to_owned(), // failure #2: malformed tag_expr
                write_ceiling: "read".to_owned(),
                budgets: Default::default(),
                instructions: None,
                enabled: true,
                aliases: BTreeMap::new(),
                // failure #3: scopes a provider this pack never declares
                auth_providers: BTreeSet::from(["ghost-provider".to_owned()]),
            },
        )]),
        tags: BTreeSet::from(["orphaned-tag".to_owned()]), // failure #4: tag vocabulary mismatch
    };
    let errors = pack::validate(&pack).unwrap_err();
    assert!(
        errors.len() >= 3,
        "expected several independent failures, got {errors:?}"
    );
}

#[test]
fn a_credential_shaped_value_is_rejected() {
    let text = std::fs::read_to_string("examples/demo.pack.yaml").expect("demo pack readable");
    let mut demo: Pack = serde_norway::from_str(&text).expect("demo pack parses");
    demo.auth_providers
        .insert("leaky".to_owned(), leaky_auth_provider());
    let errors = pack::validate(&demo).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, pack::ValidationError::Credential { .. }))
    );
}

fn leaky_auth_provider() -> api2mcp::pack::PackAuthProvider {
    api2mcp::pack::PackAuthProvider {
        service: "demo-api".to_owned(),
        kind: api2mcp::pack::PackAuthKind::StaticHeader,
        // A live-looking token where an env var *name* belongs.
        credential_env_key: "ghp_aBcDeFgHiJkLmNoPqRsT1234567890".to_owned(),
        header_name: "Authorization".to_owned(),
        value_template: "Bearer {token}".to_owned(),
        scopes: Vec::new(),
        token_url: None,
        bound_origin: "http://127.0.0.1:8089".to_owned(),
    }
}

/// The demo pack (`examples/demo.pack.yaml`, what `make seed` imports) actually runs: load it,
/// point its one service at a freshly started `tests/fixture::Fixture` — the local fixture
/// server used throughout this test suite, not a real external service — import, resolve, and
/// drive both api_calls for real over `http::bind`/`http::send`/`project::apply`.
#[tokio::test]
async fn demo_pack_resolves_and_runs_against_the_fixture() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let f = Fixture::start().await;
    f.set(
        "/items/1",
        Behavior::Json(serde_json::json!({"id": "1", "title": "One"})),
    );
    f.set(
        "/items",
        Behavior::Json(serde_json::json!({"items": [{"id": "1", "title": "One"}]})),
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

    pack::import(&stores, &demo, false).await?;
    let plan = build_plan(&stores, &slug("demo")).await?;

    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool: UpstreamPool = loopback_pool();

    // `list-items`, no projection: read the raw upstream JSON straight through.
    let list = plan
        .calls
        .get(&slug("list-items"))
        .expect("list-items planned");
    let args = api2mcp::schema::bind_args(&list.api_call.params, &serde_json::json!({}))?;
    let bound = api2mcp::http::bind(&list.api_call, &list.service, &args)?;
    let client = pool.client_for(&list.service).await?;
    let send_params = api2mcp::http::SendParams {
        allowlist: &list.service.origin_allowlist,
        policy: &policy,
        auth: None,
        max_response_bytes: list.service.max_response_bytes,
        deadline: std::time::Duration::from_secs(5),
        max_redirects: 5,
    };
    let response = api2mcp::http::send(&client, bound, send_params).await?;
    let body: serde_json::Value = serde_json::from_slice(&response.body)?;
    let first_id = body["items"][0]["id"]
        .as_str()
        .expect("first item id")
        .to_owned();

    // `get-item`, with a path param and a definer-fixed query param, projected on the way out.
    let get = plan.calls.get(&slug("get-item")).expect("get-item planned");
    let args =
        api2mcp::schema::bind_args(&get.api_call.params, &serde_json::json!({"id": first_id}))?;
    let bound = api2mcp::http::bind(&get.api_call, &get.service, &args)?;
    let client = pool.client_for(&get.service).await?;
    let send_params = api2mcp::http::SendParams {
        allowlist: &get.service.origin_allowlist,
        policy: &policy,
        auth: None,
        max_response_bytes: get.service.max_response_bytes,
        deadline: std::time::Duration::from_secs(5),
        max_redirects: 5,
    };
    let response = api2mcp::http::send(&client, bound, send_params).await?;
    let body: serde_json::Value = serde_json::from_slice(&response.body)?;
    let compiled = api2mcp::project::CompiledProjection::compile(
        get.api_call
            .projection
            .as_ref()
            .expect("get-item has a projection"),
    )?;
    let projected = api2mcp::project::apply(&compiled, &body)?;
    assert_eq!(projected["id"], serde_json::json!("1"));
    assert_eq!(projected["title"], serde_json::json!("One"));

    // The script tool is present and its declared calls resolve through this endpoint's
    // selection (I1) — executing it needs the Rhai engine, a later chunk.
    assert!(plan.tool("item-summary").is_some());
    let callable = plan
        .callable_by
        .get(&slug("item-summary"))
        .expect("script planned");
    assert_eq!(callable.get("get"), Some(&slug("get-item")));
    assert_eq!(callable.get("list"), Some(&slug("list-items")));

    // Every request went to the fixture, over loopback, with no credential in sight.
    assert_eq!(f.seen().len(), 2);

    db.teardown().await?;
    Ok(())
}
