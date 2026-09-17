//! Integration tests for the `/mcp` control plane (`server::mcp::control`): the definition-
//! authoring factory a `control_plane`-capable service token reaches at bare `POST /mcp`, as
//! opposed to `/mcp/{slug}` (the data plane, unchanged — see `tests/mcp.rs`/
//! `tests/mcp_endpoint_grants.rs`/`tests/mcp_tool_calls.rs`).
//!
//! Capability gating and the definition-lifecycle round trip. Split from
//! `tests/mcp_control_plane_auth.rs` (the I5 auth-provider-hiding tests, ownership and
//! validation) to keep both files under the workspace's 400-line cap; both share
//! `tests/mcp_control_plane_support/harness.rs`.

mod common;
mod fixture;

#[path = "mcp_control_plane_support/harness.rs"]
mod cp;
#[path = "mcp_support/harness.rs"]
mod harness;

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;

use cp::{
    api_call_body, call_tool, create_minimal_endpoint, endpoint_body, is_error, service_body,
    setup, tool_json,
};
use fixture::Behavior;
use harness::{req, rpc};

#[tokio::test]
async fn a_plain_token_is_refused_on_mcp_identically_to_an_unknown_endpoint() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let control_ok = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        req(1, "initialize", None),
    )
    .await;
    assert!(control_ok.get("result").is_some(), "got {control_ok:?}");

    let refused = rpc(
        &h.router,
        &h.plain_token,
        "/mcp",
        req(2, "initialize", None),
    )
    .await;
    let unknown = rpc(
        &h.router,
        &h.plain_token,
        "/mcp/does-not-exist",
        req(2, "initialize", None),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32001));
    assert_eq!(refused["error"]["code"], unknown["error"]["code"]);
    assert!(refused.get("result").is_none());
    // Same message template, naming "mcp" where the other names the slug it tried.
    let expected = unknown["error"]["message"]
        .as_str()
        .unwrap()
        .replace("does-not-exist", "mcp");
    assert_eq!(refused["error"]["message"], json!(expected));

    // Unrelated methods aren't gated by the capability at all — `Method not found` regardless.
    let bogus = rpc(
        &h.router,
        &h.plain_token,
        "/mcp",
        req(3, "not/a/method", None),
    )
    .await;
    assert_eq!(bogus["error"]["code"], json!(-32601));

    h.db.teardown().await
}

/// Bare `/mcp` no longer resolves any endpoint at all — a plain sanity check that it behaves as
/// the fixed control-plane surface, not as an alias for some particular curated endpoint (the
/// migrated-away behaviour `tests/mcp.rs` used to cover).
#[tokio::test]
async fn bare_mcp_never_resolves_as_an_endpoint_even_for_an_unrestricted_data_plane_token()
-> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let resp = rpc(
        &h.router,
        &h.plain_token,
        "/mcp",
        req(1, "tools/list", None),
    )
    .await;
    assert_eq!(resp["error"]["code"], json!(-32001));

    h.db.teardown().await
}

#[tokio::test]
async fn a_control_token_can_list_and_create_and_the_result_becomes_callable() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    h.fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );

    let list_before = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(1, "service.list", json!({})),
    )
    .await;
    assert_eq!(tool_json(&list_before), json!([]));

    let created = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            2,
            "service.create",
            service_body("svc-a", &h.fixture.base_url()),
        ),
    )
    .await;
    assert!(!is_error(&created), "got {created:?}");

    let created = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            3,
            "api_call.create",
            api_call_body("get-item", "svc-a", "/items/1", &["x"]),
        ),
    )
    .await;
    assert!(!is_error(&created), "got {created:?}");

    let created = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(4, "endpoint.create", endpoint_body("ep-a", "has(x)")),
    )
    .await;
    assert!(!is_error(&created), "got {created:?}");

    // The endpoint's own resolved plan sees it too.
    let plan = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(5, "endpoint.plan", json!({"slug": "ep-a"})),
    )
    .await;
    let plan_json = tool_json(&plan);
    let names: Vec<&str> = plan_json["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"get-item"), "got {plan_json:?}");

    // And it's a real, callable tool on the data plane.
    let tools_list = rpc(
        &h.router,
        &h.control_token,
        "/mcp/ep-a",
        req(6, "tools/list", None),
    )
    .await;
    let names: Vec<&str> = tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"get-item"), "got {tools_list:?}");

    let called = rpc(
        &h.router,
        &h.control_token,
        "/mcp/ep-a",
        call_tool(7, "get-item", json!({})),
    )
    .await;
    assert!(!is_error(&called), "got {called:?}");
    assert_eq!(tool_json(&called)["title"], json!("One"));

    h.db.teardown().await
}

#[tokio::test]
async fn control_plane_access_does_not_grant_data_plane_access_not_separately_granted() -> Result<()>
{
    let Some(h) = setup().await? else {
        return Ok(());
    };

    // `mint` resolves each granted endpoint slug to a real row at mint time, so both endpoints
    // must exist first.
    create_minimal_endpoint(&h.stores, h.owner_id, "off-limits").await?;
    create_minimal_endpoint(&h.stores, h.owner_id, "granted").await?;
    let scoped_token = h
        .stores
        .service_token()
        .mint(
            h.owner_id,
            "control-but-scoped".to_owned(),
            None,
            BTreeSet::from(["granted".parse().unwrap()]),
            true,
        )
        .await?;

    // The control plane itself is reachable...
    let ok = rpc(
        &h.router,
        &scoped_token.plaintext,
        "/mcp",
        call_tool(1, "endpoint.list", json!({})),
    )
    .await;
    assert!(!is_error(&ok), "got {ok:?}");

    // ...its own granted endpoint is reachable...
    let granted = rpc(
        &h.router,
        &scoped_token.plaintext,
        "/mcp/granted",
        req(2, "initialize", None),
    )
    .await;
    assert!(granted.get("result").is_some(), "got {granted:?}");

    // ...but an endpoint it wasn't separately granted is refused exactly as it would be for any
    // other endpoint-scoped token — `control_plane: true` widens nothing on the data plane.
    let refused = rpc(
        &h.router,
        &scoped_token.plaintext,
        "/mcp/off-limits",
        req(3, "initialize", None),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32001));

    h.db.teardown().await
}
