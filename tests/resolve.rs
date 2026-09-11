//! `resolve::build_plan` integration tests, against a real scratch Postgres (skipped when
//! `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`).
//!
//! One required scenario is missing here by necessity, not oversight: "an auth provider bound
//! to a foreign origin fails the plan" (I5) needs `AuthProviderStore::create`, which is
//! `pub(crate)` — I5's structural enforcement (only `pack::import`/`cli`, this crate's two
//! humans-in-the-loop, may bind a credential to an origin; see `store::auth_provider`'s module
//! doc). An integration test under `tests/` is a separate crate and cannot reach it at all, so
//! that case is covered end-to-end by a `#[cfg(test)]` unit test inside
//! `src/resolve/mod.rs` instead (plus the narrower `assert_bound`-level cases in
//! `src/resolve/auth_bind.rs`), exactly the same accommodation `tests/store.rs` documents for
//! `store::auth_provider`'s own round-trip test.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::Result;
use common::ScratchDb;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, ScriptDef, Slug, Tag, TagExpr,
};
use api2mcp::resolve::{PlanCache, ResolveError, build_plan};
use api2mcp::store::Stores;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn tag(s: &str) -> Tag {
    Tag(slug(s))
}

fn service(name: &str, allow_self: bool) -> api2mcp::model::Service {
    let base_url: url::Url = format!("https://{name}.example.com/").parse().unwrap();
    let mut origin_allowlist = BTreeSet::new();
    if allow_self {
        origin_allowlist.insert(Origin::of(&base_url).unwrap());
    }
    api2mcp::model::Service {
        slug: slug(name),
        base_url,
        origin_allowlist,
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

fn api_call(service_slug: &Slug, name: &str, access: Access) -> ApiCall {
    ApiCall {
        slug: slug(name),
        service_slug: service_slug.clone(),
        auth_provider_slug: None,
        method: http::Method::GET,
        path_template: "/things".to_owned(),
        query_fixed: BTreeMap::new(),
        body_template: None,
        access,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: vec![],
        description: None,
    }
}

fn script(name: &str, callable: BTreeMap<String, Slug>, budgets: Budgets) -> ScriptDef {
    ScriptDef {
        slug: slug(name),
        source: "()".to_owned(),
        params: vec![],
        callable,
        budgets,
        description: None,
    }
}

fn endpoint(name: &str, expr: TagExpr, write_ceiling: Access, budgets: Budgets) -> EndpointDef {
    EndpointDef {
        slug: slug(name),
        tag_expr: expr,
        write_ceiling,
        budgets,
        instructions: None,
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    }
}

#[tokio::test]
async fn resolves_exact_tool_set_and_origin_set() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-exact", true);
    stores.service().create(&svc).await?;
    let call = api_call(&svc.slug, "call-a", Access::Read);
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let hidden = api_call(&svc.slug, "call-hidden", Access::Read);
    stores
        .api_call()
        .create(&hidden, &BTreeSet::from([tag("other")]))
        .await?;
    let callable = BTreeMap::from([("helper".to_owned(), call.slug.clone())]);
    let scr = script("script-a", callable, Budgets::default());
    stores
        .script()
        .create(&scr, &BTreeSet::from([tag("expose")]))
        .await?;

    let ep = endpoint(
        "ep-exact",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let plan = build_plan(&stores, &ep.slug).await?;
    let mut names: Vec<&str> = plan.tools.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["call-a", "script-a"]);
    assert_eq!(
        plan.origins,
        BTreeSet::from([Origin::of(&svc.base_url).unwrap()])
    );

    db.teardown().await
}

#[tokio::test]
async fn origin_escaping_allowlist_fails_the_whole_plan() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-escape", false);
    stores.service().create(&svc).await?;
    let call = api_call(&svc.slug, "call-a", Access::Read);
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = endpoint(
        "ep-escape",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let err = build_plan(&stores, &ep.slug).await.unwrap_err();
    assert!(matches!(err, ResolveError::OriginNotAllowed { .. }));

    db.teardown().await
}

#[tokio::test]
async fn script_cannot_reach_a_declared_but_unselected_api_call() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-i1", true);
    stores.service().create(&svc).await?;
    let call_a = api_call(&svc.slug, "call-a", Access::Read);
    stores
        .api_call()
        .create(&call_a, &BTreeSet::from([tag("expose")]))
        .await?;
    let call_b = api_call(&svc.slug, "call-b", Access::Read);
    stores
        .api_call()
        .create(&call_b, &BTreeSet::from([tag("other")]))
        .await?;

    let callable = BTreeMap::from([
        ("a".to_owned(), call_a.slug.clone()),
        ("b".to_owned(), call_b.slug.clone()),
    ]);
    let scr = script("script-i1", callable, Budgets::default());
    stores
        .script()
        .create(&scr, &BTreeSet::from([tag("expose")]))
        .await?;

    let ep = endpoint(
        "ep-i1",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let plan = build_plan(&stores, &ep.slug).await?;
    let reachable = plan.callable_by.get(&scr.slug).expect("script planned");
    assert_eq!(reachable.get("a"), Some(&call_a.slug));
    assert_eq!(reachable.get("b"), None);

    db.teardown().await
}

#[tokio::test]
async fn read_ceiling_endpoint_refuses_a_write_api_call() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-ceiling", true);
    stores.service().create(&svc).await?;
    let call = api_call(&svc.slug, "call-w", Access::Write);
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = endpoint(
        "ep-ceiling",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let err = build_plan(&stores, &ep.slug).await.unwrap_err();
    assert!(matches!(err, ResolveError::WriteCeilingViolation { .. }));

    db.teardown().await
}

#[tokio::test]
async fn script_budget_narrows_but_never_widens_the_endpoints() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let narrow = script(
        "script-narrow",
        BTreeMap::new(),
        Budgets {
            max_calls: Some(3),
            ..Budgets::default()
        },
    );
    stores
        .script()
        .create(&narrow, &BTreeSet::from([tag("expose")]))
        .await?;
    let wide = script(
        "script-wide",
        BTreeMap::new(),
        Budgets {
            max_calls: Some(100),
            ..Budgets::default()
        },
    );
    stores
        .script()
        .create(&wide, &BTreeSet::from([tag("expose")]))
        .await?;

    let ep = endpoint(
        "ep-budget",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets {
            max_calls: Some(10),
            ..Budgets::default()
        },
    );
    stores.endpoint().create(&ep).await?;

    let plan = build_plan(&stores, &ep.slug).await?;
    // The script's own opinion (3) is narrower than the endpoint's (10) — it wins.
    assert_eq!(
        plan.tool("script-narrow")
            .expect("tool exists")
            .budgets
            .max_calls,
        Some(3)
    );
    // The script's own opinion (100) is *wider* than the endpoint's (10) — folding must never
    // let it widen the ceiling, so the endpoint's own number survives.
    assert_eq!(
        plan.tool("script-wide")
            .expect("tool exists")
            .budgets
            .max_calls,
        Some(10)
    );

    db.teardown().await
}

#[tokio::test]
async fn digest_is_stable_and_changes_with_a_definition() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-digest", true);
    stores.service().create(&svc).await?;
    let call = api_call(&svc.slug, "call-a", Access::Read);
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = endpoint(
        "ep-digest",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let plan1 = build_plan(&stores, &ep.slug).await?;
    let plan2 = build_plan(&stores, &ep.slug).await?;
    assert_eq!(plan1.digest, plan2.digest);

    let mut changed = call.clone();
    changed.path_template = "/other-things".to_owned();
    stores
        .api_call()
        .update(&changed, &BTreeSet::from([tag("expose")]))
        .await?;

    let plan3 = build_plan(&stores, &ep.slug).await?;
    assert_ne!(plan1.digest, plan3.digest);

    db.teardown().await
}

#[tokio::test]
async fn cache_returns_same_arc_until_generation_bumps() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let svc = service("svc-cache", true);
    stores.service().create(&svc).await?;
    let call = api_call(&svc.slug, "call-a", Access::Read);
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = endpoint(
        "ep-cache",
        TagExpr::Has(tag("expose")),
        Access::Read,
        Budgets::default(),
    );
    stores.endpoint().create(&ep).await?;

    let cache = PlanCache::new();
    let first = cache.get_or_build(&stores, &ep.slug).await?;
    let second = cache.get_or_build(&stores, &ep.slug).await?;
    assert!(Arc::ptr_eq(&first, &second));

    // Any definition write bumps `meta.definitions_generation` in the same transaction.
    stores
        .service()
        .create(&service("svc-cache-other", true))
        .await?;

    let third = cache.get_or_build(&stores, &ep.slug).await?;
    assert!(!Arc::ptr_eq(&first, &third));

    db.teardown().await
}
