//! `store` integration tests, against a real scratch Postgres (skipped when
//! `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`). Round-trips each aggregate
//! through the public `store::` API only; `auth_provider`'s `create`/`update`/`delete` are
//! `pub(crate)` (I5) and therefore untestable from here by design — they're covered by unit
//! tests inside `src/store/auth_provider.rs` instead.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use common::ScratchDb;
use sea_orm::EntityTrait;

use api2mcp::entity::service_tokens;
use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Param, ParamLocation, ParamType,
    Service, Slug, TagExpr,
};
use api2mcp::store::{NewUser, ServiceTokenStore, StoreError, Stores};

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
        default_headers: BTreeMap::from([("Accept".to_string(), "application/json".to_string())]),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: Some(60),
        max_response_bytes: 1_000_000,
    }
}

fn plain_param(name: &str, position: i32) -> Param {
    Param {
        name: name.to_owned(),
        location: ParamLocation::Query,
        ty: ParamType::String,
        required: false,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position,
    }
}

fn sample_api_call(
    owner_id: uuid::Uuid,
    service_slug: &Slug,
    name: &str,
    params: Vec<Param>,
) -> ApiCall {
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
        params,
        description: None,
    }
}

#[tokio::test]
async fn service_round_trips() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "acme");
    stores.service().create(&service).await?;
    let fetched = stores
        .service()
        .get_by_slug(owner_id, &service.slug)
        .await?
        .expect("service exists");
    assert_eq!(fetched, service);

    db.teardown().await
}

#[tokio::test]
async fn api_call_params_come_back_in_position_order() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "params-svc");
    stores.service().create(&service).await?;

    // Deliberately out of position order in the input Vec.
    let params = vec![
        plain_param("gamma", 2),
        plain_param("alpha", 0),
        plain_param("beta", 1),
    ];
    let api_call = sample_api_call(owner_id, &service.slug, "list", params);
    stores
        .api_call()
        .create(&api_call, &BTreeSet::new())
        .await?;

    let fetched = stores
        .api_call()
        .get(owner_id, &service.slug, &api_call.slug)
        .await?
        .expect("api_call exists");
    let names: Vec<&str> = fetched
        .api_call
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    let positions: Vec<i32> = fetched.api_call.params.iter().map(|p| p.position).collect();
    assert_eq!(positions, vec![0, 1, 2]);

    db.teardown().await
}

#[tokio::test]
async fn script_declared_call_allowlist_round_trips() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "script-svc");
    stores.service().create(&service).await?;
    let call_a = sample_api_call(owner_id, &service.slug, "call-a", vec![]);
    let call_b = sample_api_call(owner_id, &service.slug, "call-b", vec![]);
    stores.api_call().create(&call_a, &BTreeSet::new()).await?;
    stores.api_call().create(&call_b, &BTreeSet::new()).await?;

    let mut callable = BTreeMap::new();
    callable.insert("first".to_string(), call_a.slug.clone());
    callable.insert("second".to_string(), call_b.slug.clone());
    // A script's own budget opinion (I6) and its param descriptions round-trip through
    // five nullable columns and `script_params.description` respectively — see
    // `src/store/script.rs`.
    let budgets = Budgets {
        max_calls: Some(3),
        max_bytes: Some(50_000),
        wall_clock: Some(std::time::Duration::from_millis(1_500)),
        max_pages: Some(2),
        max_concurrency: Some(1),
    };
    let script = api2mcp::model::ScriptDef {
        owner_id,
        slug: slug("aggregate"),
        source: "let r = api(\"first\", #{}); r".to_owned(),
        params: vec![Param {
            location: ParamLocation::Local,
            description: Some("how many rows to fetch".to_owned()),
            ..plain_param("limit", 0)
        }],
        callable: callable.clone(),
        budgets,
        description: Some("aggregates two calls".to_owned()),
    };
    stores.script().create(&script, &BTreeSet::new()).await?;

    let fetched = stores
        .script()
        .get(owner_id, &script.slug)
        .await?
        .expect("script exists");
    assert_eq!(fetched.script.callable, callable);
    assert_eq!(fetched.script.params.len(), 1);
    assert_eq!(fetched.script.params[0].location, ParamLocation::Local);
    assert_eq!(
        fetched.script.params[0].description.as_deref(),
        Some("how many rows to fetch")
    );
    assert_eq!(fetched.script.budgets, budgets);

    db.teardown().await
}

#[tokio::test]
async fn service_token_mint_resolve_revoke_and_expiry() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "owner@example.com".to_owned(),
            password: "hunter2-hunter2".to_owned(),
        })
        .await?;

    let tokens = ServiceTokenStore::new(db.conn.clone());
    let minted = tokens
        .mint(
            user.id,
            "ci token".to_owned(),
            None,
            Default::default(),
            false,
        )
        .await?;

    // The plaintext must not appear anywhere in the persisted row.
    let raw = service_tokens::Entity::find_by_id(minted.record.id)
        .one(&db.conn)
        .await?
        .expect("row exists");
    assert_ne!(raw.token_hash, minted.plaintext);
    assert!(!raw.token_prefix.is_empty());
    assert_ne!(raw.token_prefix, minted.plaintext);

    let resolved = tokens.resolve(&minted.plaintext).await?.expect("resolves");
    assert_eq!(resolved.id, minted.record.id);

    tokens.revoke(minted.record.id).await?;
    assert!(tokens.resolve(&minted.plaintext).await?.is_none());

    let expired = tokens
        .mint(
            user.id,
            "already expired".to_owned(),
            Some(Utc::now() - ChronoDuration::seconds(1)),
            Default::default(),
            false,
        )
        .await?;
    assert!(tokens.resolve(&expired.plaintext).await?.is_none());

    db.teardown().await
}

#[tokio::test]
async fn malformed_jsonb_is_a_typed_error_not_a_panic() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "malformed-svc");
    stores.service().create(&service).await?;

    // Corrupt `origin_allowlist` directly — a plain JSON string is valid JSONB but not the
    // array of origin strings this store expects.
    use sea_orm::{ConnectionTrait, Statement};
    db.conn
        .execute(Statement::from_sql_and_values(
            db.conn.get_database_backend(),
            "UPDATE services SET origin_allowlist = '\"not-an-array\"'::jsonb WHERE slug = $1",
            [service.slug.as_str().into()],
        ))
        .await?;

    let err = stores
        .service()
        .get_by_slug(owner_id, &service.slug)
        .await
        .expect_err("malformed origin_allowlist must be a typed error");
    assert!(
        matches!(err, StoreError::Malformed(_)),
        "got {err:?} instead"
    );

    db.teardown().await
}

#[tokio::test]
async fn definitions_generation_bumps_on_a_definition_write() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let before = stores.meta().definitions_generation().await?;
    stores
        .service()
        .create(&sample_service(owner_id, "gen-svc"))
        .await?;
    let after = stores.meta().definitions_generation().await?;
    assert_eq!(after, before + 1);

    db.teardown().await
}

#[tokio::test]
async fn endpoint_round_trips_tag_expr_and_budgets() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let endpoint = EndpointDef {
        owner_id,
        slug: slug("demo"),
        tag_expr: TagExpr::And(
            Box::new(TagExpr::Has(api2mcp::model::Tag(slug("read")))),
            Box::new(TagExpr::Not(Box::new(TagExpr::Has(api2mcp::model::Tag(
                slug("deprecated"),
            ))))),
        ),
        write_ceiling: Access::Read,
        budgets: Budgets {
            max_calls: Some(10),
            max_bytes: Some(1_000_000),
            wall_clock: Some(std::time::Duration::from_secs(30)),
            max_pages: Some(5),
            max_concurrency: Some(2),
        },
        instructions: Some("demo endpoint".to_owned()),
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    };
    stores.endpoint().create(&endpoint).await?;

    let fetched = stores
        .endpoint()
        .get(owner_id, &endpoint.slug)
        .await?
        .expect("endpoint exists");
    assert_eq!(fetched, endpoint);

    db.teardown().await
}

#[tokio::test]
async fn api_call_param_description_round_trips() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let owner_id = db.create_user().await?;
    let service = sample_service(owner_id, "param-desc-svc");
    stores.service().create(&service).await?;

    let param = Param {
        description: Some("the search query".to_owned()),
        ..plain_param("q", 0)
    };
    let api_call = sample_api_call(owner_id, &service.slug, "search", vec![param]);
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
        fetched.api_call.params[0].description.as_deref(),
        Some("the search query")
    );

    db.teardown().await
}
