//! Integration tests for chunk C11's MCP data plane: `server::build_router` driven end to end
//! through [`tower::ServiceExt::oneshot`] — no bound port, exactly why `build_router` is `pub`.
//! Skipped when `TEST_DATABASE_URL` is unset, same convention as every other integration test in
//! this suite (see `tests/common/mod.rs`).
//!
//! `initialize`, protocol framing, and authentication/authorization edges. Actual `tools/call`
//! behavior (projection, run recording, `invoke` dispatch) lives in `tests/mcp_tool_calls.rs` —
//! split out, along with the shared harness (`tests/mcp_support/harness.rs`), to keep both files
//! under the workspace's 400-line cap.

mod common;
mod fixture;

#[path = "mcp_support/harness.rs"]
mod harness;

use anyhow::Result;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use tower::ServiceExt;

use harness::{post_raw, req, rpc, setup};

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

    // Bare `/mcp` is the control plane now (`server::mcp::control`), not an alias for any one
    // curated endpoint — see `tests/mcp_control_plane.rs` for its own coverage. Only the named
    // path resolves an endpoint's tools.
    let via_named = rpc(&h.router, &h.token, "/mcp/demo", req(2, "tools/list", None)).await;

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

/// A resolved-but-revoked token must never reach `tools/call` — it has to fail back at
/// `authenticate_mcp`, before `dispatch_method` even looks at the method name. There is no
/// scope check left inside the `tools/list`/`tools/call` arm any more (Decision 1 removed the
/// admin-only service token that check used to exclude), so this is now the one thing standing
/// between a bad credential and an actual tool call — worth pinning directly, not just at
/// `authenticate_mcp`'s own unit level (`tests/auth.rs`).
#[tokio::test]
async fn a_revoked_token_cannot_call_a_tool() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let record = h
        .stores
        .service_token()
        .resolve(&h.token)
        .await?
        .expect("the harness token resolves before revocation");
    h.stores.service_token().revoke(record.id).await?;

    let body = serde_json::to_vec(&req(
        1,
        "tools/call",
        Some(json!({"name": "get-item", "arguments": {"id": "1"}})),
    ))?;
    let (status, headers, value) = post_raw(&h.router, Some(&h.token), "/mcp/demo", &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {value:?}");
    assert!(
        headers.get(header::WWW_AUTHENTICATE).is_some(),
        "a revoked token must get the same framed challenge as a missing/bad one"
    );

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
