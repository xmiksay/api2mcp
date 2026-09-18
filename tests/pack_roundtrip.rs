//! `src/pack/` integration tests, against real scratch Postgres instances (skipped when
//! `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`). The headline assertion is
//! [`pack_export_reimport_reproduces_everything_but_the_auth_binding`]: export from one
//! instance, import into a completely separate one, and prove `resolve::build_plan` produces a
//! byte-identical `EndpointPlan::digest` while the service's auth binding — deliberately not
//! carried by a pack (Change 2) — is *not* reproduced. That single assertion is what proves a
//! pack is genuinely portable, not merely well-formed YAML, without overclaiming a guarantee
//! auth was never meant to have.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use common::ScratchDb;
use serde_json::json;

use api2mcp::http::{SsrfPolicy, UpstreamPool};
use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Service, Slug, Tag, TagExpr,
};
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::build_plan;
use api2mcp::store::Stores;
use fixture::harness::loopback_pool;
use fixture::{Behavior, Fixture};
use uuid::Uuid;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn tag(s: &str) -> Tag {
    Tag(slug(s))
}

fn service(owner_id: Uuid, name: &str) -> Service {
    let base_url: url::Url = format!("https://{name}.example.com/").parse().unwrap();
    Service {
        owner_id,
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
async fn seed_minimal(stores: &Stores, owner_id: Uuid, suffix: &str) -> Result<Slug> {
    let svc = service(owner_id, &format!("svc-{suffix}"));
    stores.service().create(&svc).await?;
    let call = ApiCall {
        owner_id,
        slug: slug(&format!("call-{suffix}")),
        service_slug: svc.slug.clone(),
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
        description: Some("Fetch every thing.".to_owned()),
    };
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = EndpointDef {
        owner_id,
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

/// The highest-value test, restated for Change 2: a pack carries no auth providers at all, so
/// "export → wipe → import" can no longer reproduce a service's auth binding — only everything
/// else. Seeds a service *with* a bound auth provider (created over `/api/auth_providers`, since
/// `AuthProviderStore::create` is `pub(crate)` and unreachable from this separate test crate —
/// see `store::auth_provider`'s own doc), exports, imports into a completely separate scratch
/// database (standing in for "wipe and reimport"), and proves two things at once: an identical
/// `EndpointPlan::digest`/reachable-origin set (the structural portability claim that *is* still
/// true) and a target service with **no** auth provider at all (the one thing that deliberately
/// didn't travel). The endpoint still resolves on the target either way — a provider-less
/// service is a normal state, not a build failure (Change 1).
#[tokio::test]
async fn pack_export_reimport_reproduces_everything_but_the_auth_binding() -> Result<()> {
    let Some(source) = api_support::setup().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    let source_owner = source.admin_id;
    let ep_slug = seed_minimal(&source.stores, source_owner, "authbind").await?;
    let service_slug = slug("svc-authbind");

    let provider_body = json!({
        "slug": "prov-authbind",
        "service": "svc-authbind",
        "kind": "static_header",
        "credential_env_key": "A2M_CRED_TEST_ROUNDTRIP",
        "header_name": "Authorization",
        "value_template": "Bearer {token}",
        "bound_origin": "https://svc-authbind.example.com",
    });
    let (status, body) = api_support::admin(
        &source,
        Method::POST,
        "/api/auth_providers",
        Some(provider_body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body:?}");
    assert!(
        source
            .stores
            .auth_provider()
            .get_for_service(source_owner, &service_slug)
            .await?
            .is_some(),
        "the source service must actually have a provider bound, or this test proves nothing"
    );

    let exported = pack::export_endpoint(&source.stores, source_owner, &ep_slug).await?;
    pack::validate(&exported).expect("exported pack is always valid");

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());
    let target_owner = target.create_user().await?;
    pack::import(&target_stores, &exported, false, target_owner).await?;

    // Everything but the auth binding: an identical plan digest and reachable-origin set.
    let plan_source = build_plan(&source.stores, source_owner, &ep_slug).await?;
    let plan_target = build_plan(&target_stores, target_owner, &ep_slug).await?;
    assert_eq!(plan_source.digest, plan_target.digest);
    assert_eq!(plan_source.origins, plan_target.origins);

    // The auth binding itself: deliberately not reproduced.
    assert!(
        target_stores
            .auth_provider()
            .get_for_service(target_owner, &service_slug)
            .await?
            .is_none(),
        "a pack must never carry an auth provider across the wire"
    );

    source.db.teardown().await?;
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
    let owner = db.create_user().await?;
    let ep_slug = seed_minimal(&stores, owner, "dryrun-source").await?;
    let exported = pack::export_endpoint(&stores, owner, &ep_slug).await?;

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());
    let target_owner = target.create_user().await?;

    let report = pack::import(&target_stores, &exported, true, target_owner).await?;
    assert!(
        !report.is_idempotent_no_op(),
        "a dry run into an empty db reports creates"
    );
    assert!(
        target_stores
            .endpoint()
            .get(target_owner, &ep_slug)
            .await?
            .is_none()
    );
    for slug_str in exported.services.keys() {
        assert!(
            target_stores
                .service()
                .get_by_slug(target_owner, &slug(slug_str))
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
    let owner = db.create_user().await?;
    let ep_slug = seed_minimal(&stores, owner, "idem-source").await?;
    let exported = pack::export_endpoint(&stores, owner, &ep_slug).await?;

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());
    let target_owner = target.create_user().await?;

    let first = pack::import(&target_stores, &exported, false, target_owner).await?;
    assert!(!first.is_idempotent_no_op());
    let digest_after_first = build_plan(&target_stores, target_owner, &ep_slug)
        .await?
        .digest;

    let second = pack::import(&target_stores, &exported, false, target_owner).await?;
    assert!(
        second.is_idempotent_no_op(),
        "re-importing an unchanged pack must be a no-op"
    );
    let digest_after_second = build_plan(&target_stores, target_owner, &ep_slug)
        .await?
        .digest;
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
                // failure #3: alias target this pack never declares
                aliases: BTreeMap::from([(
                    "renamed".to_owned(),
                    api2mcp::pack::PackEndpointTarget::ApiCall("ghost-call".to_owned()),
                )]),
                auth_providers: BTreeSet::new(),
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

/// A pack carries no auth providers to smuggle a credential-shaped value through any more (see
/// `pack`'s own module doc) — the heuristic still has to catch one pasted into a field a pack
/// author fully controls, like a service's own default headers.
#[test]
fn a_credential_shaped_value_is_rejected() {
    let text = std::fs::read_to_string("examples/demo.pack.yaml").expect("demo pack readable");
    let mut demo: Pack = serde_norway::from_str(&text).expect("demo pack parses");
    demo.services
        .get_mut("demo-api")
        .unwrap()
        .default_headers
        .insert(
            "X-Leaky".to_owned(),
            // A live-looking token where an ordinary header value belongs.
            "ghp_aBcDeFgHiJkLmNoPqRsT1234567890".to_owned(),
        );
    let errors = pack::validate(&demo).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, pack::ValidationError::Credential { .. }))
    );
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
    let owner = db.create_user().await?;

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

    pack::import(&stores, &demo, false, owner).await?;
    let plan = build_plan(&stores, owner, &slug("demo")).await?;

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
