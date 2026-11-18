//! `pack::import` refuses a pack whose api_call tags would turn an alias on a stored endpoint
//! outside the pack into an alias of an internal api_call — under `dry_run` too, and before
//! writing anything. Skipped when `TEST_DATABASE_URL` is unset.

mod common;

use std::collections::BTreeSet;

use anyhow::Result;
use common::ScratchDb;

use api2mcp::model::{Slug, Tag};
use api2mcp::pack::{self, ImportError, Pack};
use api2mcp::resolve::build_plan;
use api2mcp::store::Stores;
use uuid::Uuid;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

/// One service and `call-c` tagged `tag`, followed by whatever `extra` appends to the document.
fn pack_yaml(timeout_ms: u32, tag: &str, extra: &str) -> Pack {
    let pack: Pack = serde_norway::from_str(&format!(
        r#"version: 1
services:
  svc-guard:
    base_url: "https://svc-guard.example.com/"
    origin_allowlist: ["https://svc-guard.example.com"]
    timeout_ms: {timeout_ms}
    max_concurrency: 4
    max_response_bytes: 1000000
api_calls:
  call-c:
    service: svc-guard
    method: GET
    path_template: /c
    access: read
    tags: [{tag}]
{extra}
tags: [{tag}]
"#
    ))
    .expect("test pack parses");
    pack::validate(&pack).expect("the pack is valid on its own");
    pack
}

/// Stores endpoint `ep-guard` (`internal_tag_expr: has(raw)`, alias `x` -> `call-c`) with
/// `call-c` tagged `public`, so the alias is legal today.
async fn seed(stores: &Stores, owner: Uuid) -> Result<()> {
    let endpoint = r#"endpoints:
  ep-guard:
    tag_expr: has(public)
    internal_tag_expr: has(raw)
    aliases:
      x: !api_call call-c"#;
    pack::import(stores, &pack_yaml(5000, "public", endpoint), false, owner).await?;
    Ok(())
}

async fn call_c_tags(stores: &Stores, owner: Uuid) -> Result<BTreeSet<Tag>> {
    Ok(stores
        .api_call()
        .get(owner, &slug("svc-guard"), &slug("call-c"))
        .await?
        .expect("call-c exists")
        .tags)
}

fn assert_names_the_conflict(err: &ImportError) {
    assert!(matches!(err, ImportError::StoredAliasConflict(_)), "{err}");
    let text = err.to_string();
    for name in ["\"ep-guard\"", "\"x\"", "\"call-c\""] {
        assert!(text.contains(name), "{name} missing from: {text}");
    }
}

#[tokio::test]
async fn a_retag_that_breaks_a_stored_alias_is_refused_and_writes_nothing() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let owner = db.create_user().await?;
    seed(&stores, owner).await?;

    // Also changes the service, which import writes first: proof the refusal precedes every write.
    let hostile = pack_yaml(9999, "raw", "");

    let dry = pack::import(&stores, &hostile, true, owner)
        .await
        .expect_err("dry_run reports the conflict");
    assert_names_the_conflict(&dry);

    let real = pack::import(&stores, &hostile, false, owner)
        .await
        .expect_err("the import is refused");
    assert_names_the_conflict(&real);

    let service = stores
        .service()
        .get_by_slug(owner, &slug("svc-guard"))
        .await?
        .expect("service exists");
    assert_eq!(service.timeout_ms, 5000, "the service must be untouched");
    assert_eq!(
        call_c_tags(&stores, owner).await?,
        BTreeSet::from([Tag(slug("public"))]),
        "the api_call must be untouched"
    );
    build_plan(&stores, owner, &slug("ep-guard")).await?;

    db.teardown().await
}

#[tokio::test]
async fn a_pack_that_also_replaces_the_endpoint_may_retag_the_call() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let owner = db.create_user().await?;
    seed(&stores, owner).await?;

    let without_alias = r#"endpoints:
  ep-guard:
    tag_expr: has(public)
    internal_tag_expr: has(raw)"#;
    pack::import(
        &stores,
        &pack_yaml(5000, "raw", without_alias),
        false,
        owner,
    )
    .await?;

    assert_eq!(
        call_c_tags(&stores, owner).await?,
        BTreeSet::from([Tag(slug("raw"))])
    );
    let plan = build_plan(&stores, owner, &slug("ep-guard")).await?;
    assert!(plan.tool("call-c").is_none());

    db.teardown().await
}
