//! `store::service_token`'s endpoint-grant behavior against a real scratch Postgres (skipped
//! when `TEST_DATABASE_URL` is unset — see `tests/common/mod.rs`). Split out of `tests/store.rs`
//! to keep both files under the workspace's 400-line cap.

mod common;

use std::collections::BTreeSet;

use anyhow::Result;
use common::ScratchDb;
use sea_orm::EntityTrait;

use api2mcp::entity::service_token_endpoints;
use api2mcp::model::{Access, Budgets, EndpointDef, Slug, TagExpr};
use api2mcp::store::{NewUser, StoreError, Stores};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn minimal_endpoint(owner_id: uuid::Uuid, name: &str) -> EndpointDef {
    EndpointDef {
        owner_id,
        slug: slug(name),
        // No api_call carries this tag — the plan this endpoint resolves to is simply empty,
        // which is fine: these tests only need the endpoint row and its id to exist, never a
        // working plan.
        tag_expr: TagExpr::Has(api2mcp::model::Tag(slug("unused-tag"))),
        write_ceiling: Access::Read,
        budgets: Budgets::default(),
        instructions: None,
        enabled: true,
        aliases: Default::default(),
        auth_providers: BTreeSet::new(),
    }
}

#[tokio::test]
async fn a_token_minted_with_no_endpoints_is_unrestricted() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "unrestricted@example.com".to_owned(),
            password: "hunter2-hunter2".to_owned(),
        })
        .await?;

    let minted = stores
        .service_token()
        .mint(user.id, "everything".to_owned(), None, BTreeSet::new())
        .await?;
    assert!(minted.record.endpoints.is_empty());

    let resolved = stores
        .service_token()
        .resolve(&minted.plaintext)
        .await?
        .expect("resolves");
    assert!(resolved.endpoints.is_empty());

    db.teardown().await
}

#[tokio::test]
async fn a_token_minted_with_an_endpoint_carries_it_in_its_grant_set() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "restricted@example.com".to_owned(),
            password: "hunter2-hunter2".to_owned(),
        })
        .await?;
    let endpoint = minimal_endpoint(user.id, "grant-ep-a");
    stores.endpoint().create(&endpoint).await?;

    let minted = stores
        .service_token()
        .mint(
            user.id,
            "scoped".to_owned(),
            None,
            BTreeSet::from([endpoint.slug.clone()]),
        )
        .await?;
    assert_eq!(
        minted.record.endpoints,
        BTreeSet::from([endpoint.slug.clone()])
    );

    // `list_for_owner` and `get_by_id` both load the grant set the same way `mint` did.
    let listed = stores.service_token().list_for_owner(user.id).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].endpoints, BTreeSet::from([endpoint.slug]));

    db.teardown().await
}

#[tokio::test]
async fn minting_with_an_unknown_endpoint_slug_is_a_conflict_not_a_raw_fk_error() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "typo@example.com".to_owned(),
            password: "hunter2-hunter2".to_owned(),
        })
        .await?;

    let err = stores
        .service_token()
        .mint(
            user.id,
            "typo".to_owned(),
            None,
            BTreeSet::from([slug("no-such-endpoint")]),
        )
        .await
        .expect_err("an unknown endpoint slug must not silently mint");
    assert!(matches!(err, StoreError::Conflict(_)));

    db.teardown().await
}

#[tokio::test]
async fn deleting_a_granted_endpoint_leaves_no_dangling_grant_row() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "cascade@example.com".to_owned(),
            password: "hunter2-hunter2".to_owned(),
        })
        .await?;
    let endpoint = minimal_endpoint(user.id, "grant-ep-cascade");
    stores.endpoint().create(&endpoint).await?;
    let minted = stores
        .service_token()
        .mint(
            user.id,
            "will-lose-its-endpoint".to_owned(),
            None,
            BTreeSet::from([endpoint.slug.clone()]),
        )
        .await?;

    stores.endpoint().delete(user.id, &endpoint.slug).await?;

    // No row in the join table still points at the now-gone endpoint (or, since this token
    // had exactly one grant, at all) — the FK's `ON DELETE CASCADE`, not an orphaned row a
    // later `slug_by_id` lookup would have to fail on.
    let remaining = service_token_endpoints::Entity::find()
        .all(&db.conn)
        .await?;
    assert!(
        remaining.is_empty(),
        "a grant row survived its endpoint's deletion"
    );

    // And the store layer itself never trips over it: the token simply reads back
    // unrestricted now that its only grant is gone.
    let record = stores
        .service_token()
        .get_by_id(minted.record.id)
        .await?
        .expect("token still exists");
    assert!(record.endpoints.is_empty());

    db.teardown().await
}
