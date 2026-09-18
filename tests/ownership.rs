//! Per-user ownership integration tests — the scenarios the ownership feature promises: two
//! users can each own an endpoint of the same slug without colliding, a token minted by one
//! owner is refused on another owner's endpoint identically to one that doesn't exist,
//! ownership composes with a token's own endpoint grants, a script cannot resolve another
//! owner's same-slug api_call, and a pack imported under a second user produces that user's
//! own separate copy. Skipped when `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).

mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use api2mcp::config::Config;
use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, ScriptDef, Service, Slug, Tag,
    TagExpr,
};
use api2mcp::pack;
use api2mcp::resolve::{PlanCache, build_plan};
use api2mcp::server::auth::{SESSION_COOKIE_NAME, create_session};
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, Stores};

use common::ScratchDb;
use fixture::harness::loopback_pool;

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

fn api_call(owner_id: Uuid, service_slug: &Slug, name: &str, path_template: &str) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug(name),
        service_slug: service_slug.clone(),
        method: http::Method::GET,
        path_template: path_template.to_owned(),
        query_fixed: BTreeMap::new(),
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

fn minimal_endpoint(owner_id: Uuid, name: &str) -> EndpointDef {
    EndpointDef {
        owner_id,
        slug: slug(name),
        tag_expr: TagExpr::Has(tag("unused")),
        write_ceiling: Access::Read,
        budgets: Budgets::default(),
        instructions: None,
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    }
}

async fn router_for(db: &ScratchDb) -> Router {
    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("demo".to_owned()),
        "A2M_ALLOW_LOOPBACK_UPSTREAM" => Some("1".to_owned()),
        _ => None,
    })
    .expect("config");
    let state = AppState::new(
        db.conn.clone(),
        Arc::new(cfg),
        Arc::new(loopback_pool()),
        Arc::new(PlanCache::new()),
    );
    build_router(state)
}

async fn user_and_cookie(stores: &Stores, db: &ScratchDb, email: &str) -> Result<(Uuid, String)> {
    let user = stores
        .user()
        .create(NewUser {
            email: email.to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;
    let token = create_session(&db.conn, user.id, Duration::from_secs(3600)).await?;
    Ok((user.id, format!("{SESSION_COOKIE_NAME}={token}")))
}

/// A session-authenticated request against `server::api` — a smaller, local duplicate of
/// `tests/api_support::request`, kept separate since that harness carries a single fixed admin
/// user and this file specifically needs several.
async fn session_request(
    router: &Router,
    method: Method,
    path: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie);
    let bytes = match body {
        Some(v) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            serde_json::to_vec(&v).expect("serializing request body")
        }
        None => Vec::new(),
    };
    let request = builder.body(Body::from(bytes)).expect("valid request");
    let response = router.clone().oneshot(request).await.expect("router call");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading response body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn jsonrpc_req(id: i64, method: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method })
}

/// A bearer-authenticated `/mcp/{slug}` call — mirrors `tests/mcp_support::harness::rpc`, kept
/// local for the same reason `session_request` is.
async fn mcp_rpc(router: &Router, token: &str, path: &str, body: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .expect("valid request");
    let response = router.clone().oneshot(request).await.expect("router call");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("valid json-rpc response")
}

/// Scenario 1: two users each create an endpoint of the identical slug (`demo`) through the
/// admin API and both succeed; each sees only their own in a list.
#[tokio::test]
async fn two_users_can_each_own_an_endpoint_of_the_same_slug() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let router = router_for(&db).await;

    let (_alice_id, alice_cookie) = user_and_cookie(&stores, &db, "alice-demo@example.com").await?;
    let (_bob_id, bob_cookie) = user_and_cookie(&stores, &db, "bob-demo@example.com").await?;

    let body = json!({
        "slug": "demo",
        "tag_expr": "has(unused)",
        "write_ceiling": "read",
        "enabled": true,
    });

    let (status, resp) = session_request(
        &router,
        Method::POST,
        "/api/endpoints",
        &alice_cookie,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "alice: {resp:?}");

    let (status, resp) = session_request(
        &router,
        Method::POST,
        "/api/endpoints",
        &bob_cookie,
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "bob must be able to create his own `demo` too, not collide with alice's: {resp:?}"
    );

    let (status, alice_list) =
        session_request(&router, Method::GET, "/api/endpoints", &alice_cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    let alice_slugs: Vec<&str> = alice_list
        .as_array()
        .expect("array")
        .iter()
        .map(|e| e["slug"].as_str().unwrap())
        .collect();
    assert_eq!(
        alice_slugs,
        vec!["demo"],
        "alice sees only her own endpoint"
    );

    let (status, bob_list) =
        session_request(&router, Method::GET, "/api/endpoints", &bob_cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    let bob_slugs: Vec<&str> = bob_list
        .as_array()
        .expect("array")
        .iter()
        .map(|e| e["slug"].as_str().unwrap())
        .collect();
    assert_eq!(bob_slugs, vec!["demo"], "bob sees only his own endpoint");

    db.teardown().await
}

/// Scenario 2: a token minted for one owner is refused on another owner's endpoint, with a
/// response byte-for-byte identical to a genuinely nonexistent slug.
#[tokio::test]
async fn a_token_is_refused_on_another_owners_endpoint_identically_to_nonexistent() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let router = router_for(&db).await;

    let alice = stores
        .user()
        .create(NewUser {
            email: "alice-mcp@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let bob = stores
        .user()
        .create(NewUser {
            email: "bob-mcp@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    // Bob owns an endpoint Alice has no counterpart for.
    stores
        .endpoint()
        .create(&minimal_endpoint(bob.id, "bob-only"))
        .await?;

    let alice_token = stores
        .service_token()
        .mint(alice.id, "alice-token".into(), None, BTreeSet::new(), false)
        .await?;

    let on_bobs = mcp_rpc(
        &router,
        &alice_token.plaintext,
        "/mcp/bob-only",
        jsonrpc_req(1, "initialize"),
    )
    .await;
    let on_nonexistent = mcp_rpc(
        &router,
        &alice_token.plaintext,
        "/mcp/does-not-exist-at-all",
        jsonrpc_req(1, "initialize"),
    )
    .await;

    assert!(on_bobs.get("result").is_none(), "got {on_bobs:?}");
    assert_eq!(
        on_bobs["error"]["code"], on_nonexistent["error"]["code"],
        "same error code either way"
    );
    assert_eq!(on_bobs["error"]["code"], json!(-32001));
    // The message differs only in which slug it names — swap it in and they match exactly,
    // proving the two responses really are the same shape, not just coincidentally the same code.
    let expected = on_nonexistent["error"]["message"]
        .as_str()
        .unwrap()
        .replace("does-not-exist-at-all", "bob-only");
    assert_eq!(on_bobs["error"]["message"], json!(expected));

    db.teardown().await
}

/// Scenario 3: ownership and endpoint grants compose. Alice's token, granted only endpoint
/// `a`, is refused on her own `b` — and even though Bob independently owns his own endpoint
/// named `a`, Alice's token resolving `/mcp/a` always resolves *her own* `a` (proven by the
/// tool it exposes), never Bob's, because owner-scoping and grant-checking are two independent
/// gates that must both pass.
#[tokio::test]
async fn ownership_and_endpoint_grants_compose() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let router = router_for(&db).await;

    let alice = stores
        .user()
        .create(NewUser {
            email: "alice-compose@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let bob = stores
        .user()
        .create(NewUser {
            email: "bob-compose@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    // Alice's own "a" exposes a distinctly-named tool; Bob's separate "a" exposes a different
    // one, so a resolved plan's tool set proves whose definitions actually got selected.
    let alice_svc = service(alice.id, "svc-alice-compose");
    stores.service().create(&alice_svc).await?;
    let alice_call = api_call(alice.id, &alice_svc.slug, "alice-only-tool", "/alice-thing");
    stores
        .api_call()
        .create(&alice_call, &BTreeSet::from([tag("expose")]))
        .await?;
    let alice_a = EndpointDef {
        tag_expr: TagExpr::Has(tag("expose")),
        ..minimal_endpoint(alice.id, "a")
    };
    stores.endpoint().create(&alice_a).await?;
    stores
        .endpoint()
        .create(&minimal_endpoint(alice.id, "b"))
        .await?;

    let bob_svc = service(bob.id, "svc-bob-compose");
    stores.service().create(&bob_svc).await?;
    let bob_call = api_call(bob.id, &bob_svc.slug, "bob-only-tool", "/bob-thing");
    stores
        .api_call()
        .create(&bob_call, &BTreeSet::from([tag("expose")]))
        .await?;
    let bob_a = EndpointDef {
        tag_expr: TagExpr::Has(tag("expose")),
        ..minimal_endpoint(bob.id, "a")
    };
    stores.endpoint().create(&bob_a).await?;

    let scoped = stores
        .service_token()
        .mint(
            alice.id,
            "scoped-to-a".into(),
            None,
            BTreeSet::from([slug("a")]),
            false,
        )
        .await?;

    // Reaches her own granted "a" — and it is genuinely *hers*: the tool list names her own
    // api_call, never Bob's same-slug endpoint's.
    let list = mcp_rpc(
        &router,
        &scoped.plaintext,
        "/mcp/a",
        jsonrpc_req(1, "tools/list"),
    )
    .await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"alice-only-tool"),
        "expected alice's own tool, got {names:?}"
    );
    assert!(
        !names.contains(&"bob-only-tool"),
        "must never see bob's same-slug endpoint's tools: {names:?}"
    );

    // Refused on her own "b" — not in the grant set.
    let on_b = mcp_rpc(
        &router,
        &scoped.plaintext,
        "/mcp/b",
        jsonrpc_req(2, "initialize"),
    )
    .await;
    assert_eq!(on_b["error"]["code"], json!(-32001), "got {on_b:?}");

    db.teardown().await
}

/// Scenario 4: a script's `api()` binding cannot resolve an api_call of the same bare slug
/// belonging to another owner — even though both rows exist side by side in the same database.
#[tokio::test]
async fn a_scripts_api_binding_cannot_resolve_another_owners_same_slug_api_call() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let alice = stores
        .user()
        .create(NewUser {
            email: "alice-script@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let bob = stores
        .user()
        .create(NewUser {
            email: "bob-script@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    let alice_svc = service(alice.id, "svc-alice-script");
    stores.service().create(&alice_svc).await?;
    let bob_svc = service(bob.id, "svc-bob-script");
    stores.service().create(&bob_svc).await?;

    // Same bare slug, "shared", owned by two different people, resolving to different paths so
    // a resolved plan can prove which one actually got bound.
    let alice_call = api_call(alice.id, &alice_svc.slug, "shared", "/alice-thing");
    stores
        .api_call()
        .create(&alice_call, &BTreeSet::from([tag("expose")]))
        .await?;
    let bob_call = api_call(bob.id, &bob_svc.slug, "shared", "/bob-thing");
    stores
        .api_call()
        .create(&bob_call, &BTreeSet::from([tag("expose")]))
        .await?;

    // A third owner, Carol, has no api_call of her own named "shared" — only Alice and Bob do
    // — so a script of hers declaring that binding must fail outright:
    // `store::script::create` resolves `callable` scoped to the script's own owner, so it can
    // never silently pick up someone else's row of the same slug.
    let carol = stores
        .user()
        .create(NewUser {
            email: "carol-script@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let carols_broken_script = ScriptDef {
        owner_id: carol.id,
        slug: slug("carols-script"),
        source: "()".to_owned(),
        params: vec![],
        callable: BTreeMap::from([("item".to_owned(), slug("shared"))]),
        budgets: Budgets::default(),
        description: None,
    };
    stores
        .script()
        .create(&carols_broken_script, &BTreeSet::new())
        .await
        .expect_err("carol has no api_call named `shared` of her own");

    // Alice's own script naming the same bare slug resolves to *her own* api_call.
    let alice_script = ScriptDef {
        owner_id: alice.id,
        slug: slug("alice-script"),
        source: "()".to_owned(),
        params: vec![],
        callable: BTreeMap::from([("item".to_owned(), slug("shared"))]),
        budgets: Budgets::default(),
        description: None,
    };
    stores
        .script()
        .create(&alice_script, &BTreeSet::from([tag("expose")]))
        .await?;

    let ep = EndpointDef {
        tag_expr: TagExpr::Has(tag("expose")),
        ..minimal_endpoint(alice.id, "ep-cross-owner")
    };
    stores.endpoint().create(&ep).await?;

    let plan = build_plan(&stores, alice.id, &ep.slug).await?;
    let reachable = plan
        .callable_by
        .get(&alice_script.slug)
        .expect("script planned");
    assert_eq!(reachable.get("item"), Some(&alice_call.slug));
    let planned_call = plan.calls.get(&alice_call.slug).expect("api_call planned");
    assert_eq!(
        planned_call.api_call.path_template, "/alice-thing",
        "must bind alice's own api_call, never bob's same-slug one"
    );

    db.teardown().await
}

/// Scenario 5: exporting one owner's endpoint and importing it under a second user produces
/// that user's own separate copy, resolving to a working plan of its own — coexisting with,
/// not colliding with, the original.
#[tokio::test]
async fn pack_export_then_import_under_a_second_user_produces_their_own_copy() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let alice = stores
        .user()
        .create(NewUser {
            email: "alice-pack@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;
    let bob = stores
        .user()
        .create(NewUser {
            email: "bob-pack@example.com".into(),
            password: "correct horse battery staple".into(),
        })
        .await?;

    let svc = service(alice.id, "svc-export-owner");
    stores.service().create(&svc).await?;
    let call = api_call(alice.id, &svc.slug, "call-export-owner", "/things");
    stores
        .api_call()
        .create(&call, &BTreeSet::from([tag("expose")]))
        .await?;
    let ep = EndpointDef {
        tag_expr: TagExpr::Has(tag("expose")),
        ..minimal_endpoint(alice.id, "ep-export-owner")
    };
    stores.endpoint().create(&ep).await?;

    let exported = pack::export_endpoint(&stores, alice.id, &ep.slug).await?;
    pack::validate(&exported).expect("exported pack is always valid");

    let report = pack::import(&stores, &exported, false, bob.id).await?;
    assert!(!report.is_idempotent_no_op(), "bob's import creates rows");

    // Bob's own copy resolves independently, and carries the identical definition content.
    let alices_plan = build_plan(&stores, alice.id, &ep.slug).await?;
    let bobs_plan = build_plan(&stores, bob.id, &ep.slug).await?;
    assert!(bobs_plan.tool("call-export-owner").is_some());
    assert_eq!(
        alices_plan.digest, bobs_plan.digest,
        "same definition content produces the same digest for each owner's own copy"
    );

    // And each owner's own listing shows only their own row — the import created a genuinely
    // separate copy, not a shared one.
    assert_eq!(stores.service().list(alice.id).await?.len(), 1);
    assert_eq!(stores.service().list(bob.id).await?.len(), 1);

    db.teardown().await
}
