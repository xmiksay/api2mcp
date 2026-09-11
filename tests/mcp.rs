//! Integration tests for chunk C11's MCP data plane: `server::build_router` driven end to end
//! through [`tower::ServiceExt::oneshot`] — no bound port, exactly why `build_router` is `pub`.
//! Skipped when `TEST_DATABASE_URL` is unset, same convention as every other integration test in
//! this suite (see `tests/common/mod.rs`).
//!
//! Each test imports `examples/demo.pack.yaml` (patched to point at a freshly started
//! `tests/fixture::Fixture`, exactly like `tests/pack_roundtrip.rs`'s
//! `demo_pack_resolves_and_runs_against_the_fixture`) into its own scratch database, mints a
//! `mcp`-scoped service token, and drives the resulting endpoint over real HTTP-shaped requests.

mod common;
mod fixture;

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use api2mcp::config::Config;
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::PlanCache;
use api2mcp::server::{AppState, build_router};
use api2mcp::store::{NewUser, RunStatus, Stores};

use common::ScratchDb;
use fixture::harness::loopback_pool;
use fixture::{Behavior, Fixture};

/// Everything a test needs: the scratch db (kept alive so `stores()`-backed assertions after the
/// HTTP round trip can still query it, and so `teardown` can run at the end), the fixture
/// upstream, the built router, and a plaintext `mcp`-scoped service token.
struct Harness {
    db: ScratchDb,
    stores: Stores,
    router: Router,
    token: String,
}

async fn setup() -> Result<Option<Harness>> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(None);
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());

    let user = stores
        .user()
        .create(NewUser {
            email: "mcp-test@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
            is_admin: false,
        })
        .await?;
    let minted = stores
        .service_token()
        .mint(
            user.id,
            "mcp test token".to_owned(),
            vec!["mcp".to_owned()],
            None,
        )
        .await?;

    let fixture = Fixture::start().await;
    fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );
    fixture.set(
        "/items",
        Behavior::Json(json!({"items": [{"id": "1", "title": "One"}]})),
    );

    let text = std::fs::read_to_string("examples/demo.pack.yaml")?;
    let mut demo: Pack = serde_norway::from_str(&text)?;
    let svc = demo
        .services
        .get_mut("demo-api")
        .expect("demo-api in the pack");
    svc.base_url = fixture.base_url().to_string();
    svc.origin_allowlist = BTreeSet::from([fixture.base_url().to_string()]);
    pack::validate(&demo).expect("patched demo pack is still valid");
    pack::import(&stores, &demo, false).await?;

    let cfg = Config::from_lookup(|k| match k {
        "DATABASE_URL" => Some("postgres://unused/unused".to_owned()),
        "A2M_DEFAULT_ENDPOINT" => Some("demo".to_owned()),
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

    Ok(Some(Harness {
        db,
        stores,
        router,
        token: minted.plaintext,
    }))
}

/// Sends a raw body to `path`, optionally bearer-authenticated, and returns the status, response
/// headers, and the body parsed as JSON (`Value::Null` for an empty body — the
/// `notifications/initialized` 202 case).
async fn post_raw(
    router: &Router,
    token: Option<&str>,
    path: &str,
    body: &[u8],
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let request = builder
        .body(Body::from(body.to_vec()))
        .expect("valid request");
    let response = router.clone().oneshot(request).await.expect("router call");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("reading response body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("valid json response body")
    };
    (status, headers, value)
}

async fn rpc(router: &Router, token: &str, path: &str, body: Value) -> Value {
    let (status, _headers, value) = post_raw(
        router,
        Some(token),
        path,
        &serde_json::to_vec(&body).expect("serializing request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected status: {value:?}");
    value
}

fn req(id: i64, method: &str, params: Option<Value>) -> Value {
    let mut obj = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let Some(p) = params {
        obj["params"] = p;
    }
    obj
}

#[tokio::test]
async fn initialize_returns_protocol_version_and_capabilities() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let resp = rpc(&h.router, &h.token, "/mcp/demo", req(1, "initialize", None)).await;
    assert_eq!(resp["result"]["protocolVersion"], json!("2025-03-26"));
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert!(
        resp["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("api2mcp")
    );

    h.db.teardown().await
}

#[tokio::test]
async fn tools_list_returns_exactly_the_tag_selected_tools_with_fixed_params_absent() -> Result<()>
{
    let Some(h) = setup().await? else {
        return Ok(());
    };

    // Bare `/mcp` resolves to `cfg.default_endpoint` ("demo") — checked against the same
    // response the named path gives.
    let via_default = rpc(&h.router, &h.token, "/mcp", req(1, "tools/list", None)).await;
    let via_named = rpc(&h.router, &h.token, "/mcp/demo", req(2, "tools/list", None)).await;
    assert_eq!(via_default["result"], via_named["result"]);

    let tools = via_named["result"]["tools"]
        .as_array()
        .expect("tools array");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "get-item",
            "invoke",
            "item-summary",
            "list-items",
            "list_tools"
        ]
    );

    let get_item = tools
        .iter()
        .find(|t| t["name"] == "get-item")
        .expect("get-item listed");
    let properties = get_item["inputSchema"]["properties"]
        .as_object()
        .expect("properties object");
    assert!(
        properties.contains_key("id"),
        "caller-supplied param present"
    );
    assert!(
        !properties.contains_key("format"),
        "definer-fixed param must never be model-visible"
    );
    assert_eq!(get_item["inputSchema"]["required"], json!(["id"]));

    h.db.teardown().await
}

#[tokio::test]
async fn tools_call_on_a_real_api_call_returns_a_projected_result_and_writes_a_runs_row()
-> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let resp = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            1,
            "tools/call",
            Some(json!({"name": "get-item", "arguments": {"id": "1"}})),
        ),
    )
    .await;
    assert!(
        resp["error"].is_null(),
        "unexpected protocol error: {resp:?}"
    );
    assert!(
        resp["result"]["isError"].is_null(),
        "unexpected tool failure: {resp:?}"
    );
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let projected: Value = serde_json::from_str(text).expect("projected value is json");
    assert_eq!(projected["id"], json!("1"));
    assert_eq!(projected["title"], json!("One"));

    let runs = h
        .stores
        .run()
        .list_for_endpoint(&"demo".parse().unwrap(), 10)
        .await?;
    assert_eq!(runs.len(), 1, "exactly one run should have been recorded");
    assert_eq!(runs[0].tool_name, "get-item");
    assert_eq!(runs[0].status, RunStatus::Ok);

    h.db.teardown().await
}

#[tokio::test]
async fn a_tool_failure_is_200_with_is_error_true() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    // `id` is required (see demo.pack.yaml) — omitting it fails argument binding, which is a
    // *tool* failure (bad arguments for this call), never a JSON-RPC protocol error.
    let resp = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            1,
            "tools/call",
            Some(json!({"name": "get-item", "arguments": {}})),
        ),
    )
    .await;
    assert!(
        resp["error"].is_null(),
        "a bad-argument tool call must not be a protocol error"
    );
    assert_eq!(resp["result"]["isError"], json!(true));
    assert!(
        !resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .is_empty()
    );

    h.db.teardown().await
}

#[tokio::test]
async fn unknown_method_is_method_not_found_and_a_malformed_body_is_a_framed_parse_error()
-> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let unknown = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(1, "not/a/real/method", None),
    )
    .await;
    assert_eq!(unknown["error"]["code"], json!(-32601));

    let (status, _headers, malformed) = post_raw(
        &h.router,
        Some(&h.token),
        "/mcp/demo",
        b"{ this is not json",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(malformed["error"]["code"], json!(-32700));

    h.db.teardown().await
}

#[tokio::test]
async fn invoke_dispatches_to_the_same_tool_as_a_direct_call() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let direct = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            1,
            "tools/call",
            Some(json!({"name": "get-item", "arguments": {"id": "1"}})),
        ),
    )
    .await;
    let via_invoke = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            2,
            "tools/call",
            Some(json!({
                "name": "invoke",
                "arguments": {"tool_name": "get-item", "args": {"id": "1"}}
            })),
        ),
    )
    .await;

    assert_eq!(
        direct["result"]["content"][0]["text"],
        via_invoke["result"]["content"][0]["text"]
    );

    h.db.teardown().await
}

#[tokio::test]
async fn a_missing_or_bad_token_gives_a_framed_401_naming_resource_metadata() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let body = serde_json::to_vec(&req(1, "initialize", None))?;

    for token in [None, Some("garbage-token-value")] {
        let (status, headers, value) = post_raw(&h.router, token, "/mcp/demo", &body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .expect("WWW-Authenticate header present");
        assert!(challenge.contains("resource_metadata="));
        assert!(challenge.contains(".well-known/oauth-protected-resource"));
        assert!(value["error"].is_object());
    }

    h.db.teardown().await
}

#[tokio::test]
async fn an_api_path_miss_returns_404_not_the_spa_shell() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let request = Request::builder()
        .method("GET")
        .uri("/api/does-not-exist")
        .body(Body::empty())?;
    let response = h.router.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    h.db.teardown().await
}
