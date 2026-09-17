//! `api_calls.description` store round-trip — split out of `tests/store.rs` purely to keep
//! that file under the workspace's 400-line cap (the file was already at the limit).
//!
//! This is the field `server::mcp::registry::describe` uses verbatim as an MCP tool's
//! `description` when present, so "does it survive a write/read cycle, and does an absent one
//! come back as `None` rather than `\"\"`" is worth its own focused test.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use common::ScratchDb;

use api2mcp::model::{Access, ApiCall, Origin, Pagination, Service, Slug};
use api2mcp::store::Stores;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn sample_service(owner_id: uuid::Uuid, name: &str) -> Service {
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

fn sample_api_call(owner_id: uuid::Uuid, service_slug: &Slug, name: &str) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug(name),
        service_slug: service_slug.clone(),
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
        description: None,
    }
}

#[tokio::test]
async fn api_call_description_round_trips() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "call-desc-svc");
    stores.service().create(&service).await?;

    let mut api_call = sample_api_call(owner_id, &service.slug, "get-thing");
    api_call.description = Some("Fetch one thing by id; returns its title and owner.".to_owned());
    stores
        .api_call()
        .create(&api_call, &BTreeSet::new())
        .await?;

    let fetched = stores
        .api_call()
        .get(owner_id, &service.slug, &api_call.slug)
        .await?
        .expect("api_call exists");
    assert_eq!(
        fetched.api_call.description.as_deref(),
        Some("Fetch one thing by id; returns its title and owner.")
    );

    db.teardown().await
}

#[tokio::test]
async fn absent_api_call_description_comes_back_as_none_not_empty_string() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "call-nodesc-svc");
    stores.service().create(&service).await?;

    // `sample_api_call` leaves `description: None` — the same "no description" state a
    // definer who never set one leaves behind. This is what tells `registry::describe` to
    // fall back to its synthesized method/path string instead.
    let undocumented = sample_api_call(owner_id, &service.slug, "get-other");
    stores
        .api_call()
        .create(&undocumented, &BTreeSet::new())
        .await?;

    let fetched = stores
        .api_call()
        .get(owner_id, &service.slug, &undocumented.slug)
        .await?
        .expect("api_call exists");
    assert_eq!(fetched.api_call.description, None);

    db.teardown().await
}
