//! `m0010_base_url_path` against real rows: written at schema version m0009 (before `http::bind`
//! prepended the base path), then migrated, then read back through the stores.

mod common;

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use api2mcp::migration::Migrator;
use api2mcp::model::{Access, ApiCall, Origin, Pagination, Service, Slug};
use api2mcp::store::Stores;
use common::ScratchDb;
use sea_orm_migration::MigratorTrait;

/// Every migration up to and including m0009 — the schema this data was written under.
const BEFORE_M0010: u32 = 9;

#[tokio::test]
async fn m0010_strips_a_spelled_out_base_path_and_the_base_url_query() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    Migrator::up(&db.conn, Some(BEFORE_M0010))
        .await
        .context("migrating up to m0009")?;
    let stores = Stores::new(db.conn.clone());
    let owner_id = db.create_user().await?;

    let prefixed = service(
        owner_id,
        "svc-prefixed",
        "https://h.example.com/api/v1?key=1#top",
    );
    let bare = service(owner_id, "svc-bare", "https://h.example.com/");
    stores.service().create(&prefixed).await?;
    stores.service().create(&bare).await?;

    let cases = [
        // (service, slug, template before, template after)
        (
            &prefixed,
            "spelled-out",
            "/api/v1/tenants/{id}",
            "/tenants/{id}",
        ),
        // The one lossy rewrite: `/` now requests `/api/v1/`, where `/api/v1` requested
        // `/api/v1` before. Closest expressible; m0010 logs a warning for it.
        (&prefixed, "exactly-base", "/api/v1", "/"),
        (
            &prefixed,
            "parameterised-prefix",
            "/api/{ver}/x",
            "/api/{ver}/x",
        ),
        (&prefixed, "issue-6-broken", "/tenants", "/tenants"),
        (&prefixed, "not-a-boundary", "/api/v10/x", "/api/v10/x"),
        (&bare, "bare-base", "/api/v1/tenants", "/api/v1/tenants"),
    ];
    for (svc, name, before, _) in &cases {
        stores
            .api_call()
            .create(
                &api_call(owner_id, &svc.slug, name, before),
                &BTreeSet::new(),
            )
            .await?;
    }

    Migrator::up(&db.conn, None)
        .await
        .context("applying m0010")?;

    let migrated = stores
        .service()
        .get_by_slug(owner_id, &prefixed.slug)
        .await?
        .context("prefixed service survives")?;
    assert_eq!(migrated.base_url.as_str(), "https://h.example.com/api/v1");
    let untouched = stores
        .service()
        .get_by_slug(owner_id, &bare.slug)
        .await?
        .context("bare service survives")?;
    assert_eq!(untouched.base_url.as_str(), "https://h.example.com/");

    for (svc, name, _, after) in &cases {
        let call = stores
            .api_call()
            .get(owner_id, &svc.slug, &name.parse()?)
            .await?
            .context("api_call survives")?;
        assert_eq!(call.api_call.path_template, *after, "api_call {name}");
    }

    db.teardown().await
}

fn service(owner_id: uuid::Uuid, name: &str, base_url: &str) -> Service {
    let base_url: url::Url = base_url.parse().expect("valid url");
    Service {
        owner_id,
        slug: name.parse().expect("valid slug"),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url).expect("valid origin")]),
        base_url,
        default_headers: Default::default(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

fn api_call(owner_id: uuid::Uuid, service_slug: &Slug, name: &str, template: &str) -> ApiCall {
    ApiCall {
        owner_id,
        slug: name.parse().expect("valid slug"),
        service_slug: service_slug.clone(),
        method: http::Method::GET,
        path_template: template.to_owned(),
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
