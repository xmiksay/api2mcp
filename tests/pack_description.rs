//! api_call `description` export/import round trip — split out of `tests/pack_roundtrip.rs`
//! purely to keep that file under the workspace's 400-line cap (the file was already at the
//! limit). See that file's module doc for the shared shape this mirrors.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use common::ScratchDb;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Service, Slug, Tag, TagExpr,
};
use api2mcp::pack;
use api2mcp::store::Stores;

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

/// One service, one tagged (and documented) api_call, one endpoint selecting it.
async fn seed(stores: &Stores) -> Result<Slug> {
    let svc = service("svc-desc");
    stores.service().create(&svc).await?;
    let call = ApiCall {
        slug: slug("call-desc"),
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
        description: Some("Fetch every thing.".to_owned()),
    };
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = EndpointDef {
        slug: slug("ep-desc"),
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

/// An api_call's `description` is what a model reads to decide whether and how to call the
/// resulting tool — it has to survive both halves of the round trip: into the exported YAML,
/// and back out through import into a separate database.
#[tokio::test]
async fn api_call_description_survives_export_and_import() -> Result<()> {
    let Some(source) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    source.migrate_up().await?;
    let source_stores = Stores::new(source.conn.clone());
    let ep_slug = seed(&source_stores).await?;

    let exported = pack::export_endpoint(&source_stores, &ep_slug).await?;
    let packed_call = exported
        .api_calls
        .get("call-desc")
        .expect("call-desc exported");
    assert_eq!(
        packed_call.description.as_deref(),
        Some("Fetch every thing.")
    );

    let Some(target) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    target.migrate_up().await?;
    let target_stores = Stores::new(target.conn.clone());
    pack::import(&target_stores, &exported, false).await?;

    let fetched = target_stores
        .api_call()
        .get(&slug("svc-desc"), &slug("call-desc"))
        .await?
        .expect("api_call imported");
    assert_eq!(
        fetched.api_call.description.as_deref(),
        Some("Fetch every thing.")
    );

    source.teardown().await?;
    target.teardown().await?;
    Ok(())
}
