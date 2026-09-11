//! `tools/call` behavior for chunk C11's MCP data plane — projection, run recording, tool
//! failure shape, and the `invoke` dispatcher tool. Split out of `tests/mcp.rs` (which keeps
//! `initialize`/protocol-framing/auth edge cases) purely to keep both files under the
//! workspace's 400-line cap; the shared harness lives in `tests/mcp_support/harness.rs`.
//! Skipped when `TEST_DATABASE_URL` is unset (see `tests/common/mod.rs`).

mod common;
mod fixture;

#[path = "mcp_support/harness.rs"]
mod harness;

use anyhow::Result;
use serde_json::{Value, json};

use api2mcp::store::RunStatus;

use harness::{req, rpc, setup};

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
