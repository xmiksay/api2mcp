//! `endpoints.internal_tag_expr` (#7), end to end over `/mcp/{slug}`: an api_call it selects is
//! reachable from the endpoint's scripts (through their `callable`, I1) but is neither listed by
//! `tools/list` nor callable by `tools/call`/`invoke` — even when `tag_expr` selects it too — and
//! it is still bounded by the endpoint's `write_ceiling`; an alias naming one fails the plan. Reuses `tests/mcp_support/harness.rs`'s imported demo pack, then
//! re-tags its two api_calls so only the internal expression selects them. Skipped when
//! `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).

mod common;
mod fixture;

#[path = "mcp_support/harness.rs"]
mod harness;

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use api2mcp::model::{Access, Slug, Tag, TagExpr};
use api2mcp::resolve::{ResolveError, build_plan};

use harness::{Harness, req, rpc, setup};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn has(t: &str) -> TagExpr {
    TagExpr::Has(Tag(slug(t)))
}

/// Moves `get-item`/`list-items` from the `demo` tag to `raw`, and points the `demo` endpoint's
/// `internal_tag_expr` at `raw` — so `tag_expr: has(demo)` now selects only the
/// `item-summary` script, whose `callable` names both api_calls.
async fn make_api_calls_internal(h: &Harness) -> Result<()> {
    let service = slug("demo-api");
    for name in ["get-item", "list-items"] {
        let tagged = h
            .stores
            .api_call()
            .get(h.owner_id, &service, &slug(name))
            .await?
            .expect("demo api_call exists");
        h.stores
            .api_call()
            .update(&tagged.api_call, &BTreeSet::from([Tag(slug("raw"))]))
            .await?;
    }
    let mut endpoint = h
        .stores
        .endpoint()
        .get(h.owner_id, &slug("demo"))
        .await?
        .expect("demo endpoint exists");
    endpoint.internal_tag_expr = Some(has("raw"));
    h.stores.endpoint().update(&endpoint).await?;
    Ok(())
}

fn tool_names(resp: &Value) -> Vec<String> {
    let mut names: Vec<String> = resp["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_owned())
        .collect();
    names.sort_unstable();
    names
}

#[tokio::test]
async fn tools_list_omits_internal_api_calls() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    make_api_calls_internal(&h).await?;

    let resp = rpc(&h.router, &h.token, "/mcp/demo", req(1, "tools/list", None)).await;
    assert_eq!(
        tool_names(&resp),
        vec!["invoke", "item-summary", "list_tools"]
    );

    h.db.teardown().await
}

#[tokio::test]
async fn a_script_can_call_an_internal_api_call() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    make_api_calls_internal(&h).await?;

    let resp = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            1,
            "tools/call",
            Some(json!({"name": "item-summary", "arguments": {}})),
        ),
    )
    .await;
    assert!(resp["error"].is_null(), "protocol error: {resp:?}");
    assert!(
        resp["result"]["isError"].is_null(),
        "tool failure: {resp:?}"
    );
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let value: Value = serde_json::from_str(text)?;
    assert_eq!(value["count"], json!(1));
    assert_eq!(value["first"]["title"], json!("One"));

    h.db.teardown().await
}

#[tokio::test]
async fn tools_call_and_invoke_refuse_an_internal_api_call() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    make_api_calls_internal(&h).await?;

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
    assert_eq!(direct["error"]["code"], json!(-32602), "{direct:?}");
    assert!(direct["result"].is_null());

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
    assert_eq!(via_invoke["error"]["code"], json!(-32602), "{via_invoke:?}");

    // Nothing reached the upstream, so nothing was recorded.
    let runs = h
        .stores
        .run()
        .list_for_endpoint(h.owner_id, &slug("demo"), 10)
        .await?;
    assert!(runs.is_empty(), "a refused call must not record a run");

    h.db.teardown().await
}

#[tokio::test]
async fn write_ceiling_still_bounds_internal_api_calls() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    make_api_calls_internal(&h).await?;

    let mut tagged = h
        .stores
        .api_call()
        .get(h.owner_id, &slug("demo-api"), &slug("list-items"))
        .await?
        .expect("list-items exists");
    tagged.api_call.access = Access::Write;
    h.stores
        .api_call()
        .update(&tagged.api_call, &BTreeSet::from([Tag(slug("raw"))]))
        .await?;

    // `demo` has `write_ceiling: read`; an internal write call fails the whole plan exactly as
    // an exposed one would.
    let err = build_plan(&h.stores, h.owner_id, &slug("demo"))
        .await
        .expect_err("a write internal api_call above a read ceiling");
    assert_eq!(
        err,
        ResolveError::WriteCeilingViolation {
            api_call: "list-items".to_owned()
        }
    );

    h.db.teardown().await
}

#[tokio::test]
async fn internal_wins_when_both_expressions_select_an_api_call() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    // Untouched demo pack: everything is tagged `demo`, so both expressions select both
    // api_calls. Fails closed: they become internal, and only the script stays a tool.
    let mut endpoint = h
        .stores
        .endpoint()
        .get(h.owner_id, &slug("demo"))
        .await?
        .expect("demo endpoint exists");
    endpoint.internal_tag_expr = Some(has("demo"));
    h.stores.endpoint().update(&endpoint).await?;

    let list = rpc(&h.router, &h.token, "/mcp/demo", req(1, "tools/list", None)).await;
    assert_eq!(
        tool_names(&list),
        vec!["invoke", "item-summary", "list_tools"]
    );

    let call = rpc(
        &h.router,
        &h.token,
        "/mcp/demo",
        req(
            2,
            "tools/call",
            Some(json!({"name": "get-item", "arguments": {"id": "1"}})),
        ),
    )
    .await;
    assert_eq!(call["error"]["code"], json!(-32602), "{call:?}");

    h.db.teardown().await
}

#[tokio::test]
async fn an_alias_of_an_internal_api_call_fails_the_plan() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    make_api_calls_internal(&h).await?;
    let mut endpoint = h
        .stores
        .endpoint()
        .get(h.owner_id, &slug("demo"))
        .await?
        .expect("demo endpoint exists");
    endpoint.aliases.insert(
        "fetch".to_owned(),
        api2mcp::model::EndpointTarget::ApiCall(slug("get-item")),
    );
    h.stores.endpoint().update(&endpoint).await?;

    let err = build_plan(&h.stores, h.owner_id, &slug("demo"))
        .await
        .expect_err("an alias of an internal api_call");
    assert_eq!(
        err,
        ResolveError::AliasOfInternalCall {
            alias: "fetch".to_owned(),
            api_call: "get-item".to_owned(),
        }
    );

    h.db.teardown().await
}
