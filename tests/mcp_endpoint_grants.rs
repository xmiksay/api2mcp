//! Integration tests for a self-service token's endpoint restriction
//! (`server::auth::authenticate_mcp`'s grant set, enforced in `server::mcp::resolve_plan`):
//! an empty grant list reaches every endpoint, a non-empty one is refused on anything else,
//! and that refusal is byte-for-byte the same response a genuinely nonexistent endpoint slug
//! gets. Split out of `tests/mcp.rs` (its own harness needs two real endpoints and two
//! differently-scoped tokens, which `mcp_support::harness::setup`'s single-endpoint/single-
//! unrestricted-token fixture doesn't provide) to keep both files under the workspace's
//! 400-line cap.

mod common;
mod fixture;

#[path = "mcp_support/harness.rs"]
mod harness;

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;

use api2mcp::config::Config;
use api2mcp::model::{Access, Budgets, EndpointDef, Slug, Tag, TagExpr};
use api2mcp::resolve::PlanCache;
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, Stores};

use common::ScratchDb;
use fixture::harness::loopback_pool;
use harness::{req, rpc};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

/// An endpoint whose tag expression selects nothing — fine here, since every test in this
/// file only calls `initialize`, which needs a resolvable endpoint row, not a working tool.
fn minimal_endpoint(owner_id: uuid::Uuid, name: &str) -> EndpointDef {
    EndpointDef {
        owner_id,
        slug: slug(name),
        tag_expr: TagExpr::Has(Tag(slug("unused-tag"))),
        write_ceiling: Access::Read,
        budgets: Budgets::default(),
        instructions: None,
        enabled: true,
        aliases: Default::default(),
        auth_providers: BTreeSet::new(),
    }
}

struct Grants {
    db: ScratchDb,
    router: axum::Router,
    unrestricted_token: String,
    scoped_token: String,
    owner_id: uuid::Uuid,
}

/// Seeds two endpoints (`grant-a`, `grant-b`), an unrestricted token, and a token restricted
/// to `grant-a` only.
async fn setup() -> Result<Option<Grants>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "grants@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;

    let ep_a = minimal_endpoint(user.id, "grant-a");
    let ep_b = minimal_endpoint(user.id, "grant-b");
    stores.endpoint().create(&ep_a).await?;
    stores.endpoint().create(&ep_b).await?;

    let unrestricted = stores
        .service_token()
        .mint(user.id, "unrestricted".to_owned(), None, BTreeSet::new())
        .await?;
    let scoped = stores
        .service_token()
        .mint(
            user.id,
            "scoped-to-a".to_owned(),
            None,
            BTreeSet::from([ep_a.slug.clone()]),
        )
        .await?;

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("grant-a".to_owned()),
        "A2M_ALLOW_LOOPBACK_UPSTREAM" => Some("1".to_owned()),
        _ => None,
    })?;
    let state = AppState::new(
        db.conn.clone(),
        Arc::new(cfg),
        Arc::new(loopback_pool()),
        Arc::new(PlanCache::new()),
    );
    let router = build_router(state);

    Ok(Some(Grants {
        db,
        router,
        unrestricted_token: unrestricted.plaintext,
        scoped_token: scoped.plaintext,
        owner_id: user.id,
    }))
}

#[tokio::test]
async fn an_unrestricted_token_reaches_every_endpoint() -> Result<()> {
    let Some(g) = setup().await? else {
        return Ok(());
    };

    for path in ["/mcp/grant-a", "/mcp/grant-b"] {
        let resp = rpc(
            &g.router,
            &g.unrestricted_token,
            path,
            req(1, "initialize", None),
        )
        .await;
        assert!(
            resp.get("result").is_some(),
            "expected {path} to succeed for an unrestricted token, got {resp:?}"
        );
    }

    g.db.teardown().await
}

#[tokio::test]
async fn a_scoped_token_reaches_its_granted_endpoint() -> Result<()> {
    let Some(g) = setup().await? else {
        return Ok(());
    };

    let resp = rpc(
        &g.router,
        &g.scoped_token,
        "/mcp/grant-a",
        req(1, "initialize", None),
    )
    .await;
    assert!(resp.get("result").is_some(), "got {resp:?}");

    g.db.teardown().await
}

#[tokio::test]
async fn a_scoped_token_is_refused_on_another_endpoint_identically_to_one_that_does_not_exist()
-> Result<()> {
    let Some(g) = setup().await? else {
        return Ok(());
    };

    let on_ungranted = rpc(
        &g.router,
        &g.scoped_token,
        "/mcp/grant-b",
        req(1, "initialize", None),
    )
    .await;
    let on_nonexistent = rpc(
        &g.router,
        &g.scoped_token,
        "/mcp/does-not-exist",
        req(1, "initialize", None),
    )
    .await;

    // Same shape as a genuinely missing endpoint, id included — an attacker probing slugs
    // must not be able to tell "exists but not granted" from "never existed" apart.
    assert_eq!(
        on_ungranted["error"]["code"],
        on_nonexistent["error"]["code"]
    );
    assert_eq!(on_ungranted["error"]["code"], serde_json::json!(-32001));
    assert!(on_ungranted.get("result").is_none());

    // And, since the slug is baked into the message on both sides, they naturally differ only
    // in which slug they name — swap it in and the two messages match exactly.
    let expected_for_b = on_nonexistent["error"]["message"]
        .as_str()
        .unwrap()
        .replace("does-not-exist", "grant-b");
    assert_eq!(
        on_ungranted["error"]["message"],
        serde_json::json!(expected_for_b)
    );

    g.db.teardown().await
}

#[tokio::test]
async fn a_scoped_token_is_also_refused_on_tools_list_for_an_ungranted_endpoint() -> Result<()> {
    let Some(g) = setup().await? else {
        return Ok(());
    };

    let resp = rpc(
        &g.router,
        &g.scoped_token,
        "/mcp/grant-b",
        req(1, "tools/list", None),
    )
    .await;
    assert_eq!(resp["error"]["code"], serde_json::json!(-32001));

    g.db.teardown().await
}

#[tokio::test]
async fn deleting_a_scoped_tokens_only_endpoint_does_not_widen_it_to_every_endpoint() -> Result<()>
{
    let Some(g) = setup().await? else {
        return Ok(());
    };
    let stores = Stores::new(g.db.conn.clone());

    // The scoped token is granted `grant-a` and nothing else. Deleting `grant-a` cascades its
    // grant row away, leaving the token with an empty grant set — which must mean "reaches
    // nothing", not "reaches everything". Under an empty-means-all reading this delete would hand
    // the token `grant-b`, escalating it as a side effect of an unrelated edit.
    stores
        .endpoint()
        .delete(g.owner_id, &slug("grant-a"))
        .await?;

    let resp = rpc(
        &g.router,
        &g.scoped_token,
        "/mcp/grant-b",
        req(1, "initialize", None),
    )
    .await;
    assert!(
        resp.get("result").is_none(),
        "a scoped token must not reach grant-b after its own endpoint was deleted: {resp:?}"
    );

    // And the unrestricted token is unaffected — this is about the scoped one widening, not about
    // the delete breaking everything.
    let ok = rpc(
        &g.router,
        &g.unrestricted_token,
        "/mcp/grant-b",
        req(2, "initialize", None),
    )
    .await;
    assert!(ok.get("result").is_some(), "got {ok:?}");

    g.db.teardown().await
}
