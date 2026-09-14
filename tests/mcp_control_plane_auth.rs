//! Integration tests for the `/mcp` control plane's I5 posture (`server::mcp::control`'s own
//! module doc): auth providers are invisible here entirely, ownership scoping, and validation
//! parity with `/api`. Split from `tests/mcp_control_plane.rs` (capability gating and the
//! definition-lifecycle round trip) to keep both files under the workspace's 400-line cap; both
//! share `tests/mcp_control_plane_support/harness.rs`.

mod common;
mod fixture;

#[path = "mcp_control_plane_support/harness.rs"]
mod cp;
#[path = "mcp_support/harness.rs"]
mod harness;

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use api2mcp::store::NewUser;

use cp::{api_call_body, call_tool, endpoint_body, is_error, service_body, setup, tool_json};
use fixture::Behavior;
use harness::{req, rpc};

#[tokio::test]
async fn no_tool_or_field_on_the_control_plane_ever_mentions_an_auth_provider() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let list = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        req(1, "tools/list", None),
    )
    .await;
    let text = list.to_string();
    assert!(
        !text.contains("auth_provider"),
        "the control plane's tool list must never mention auth providers: {text}"
    );

    h.fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );
    rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            2,
            "service.create",
            service_body("svc-b", &h.fixture.base_url()),
        ),
    )
    .await;

    // Sending `auth_provider` anyway (a field the schema never advertises) must still not
    // attach one — this tool can neither see nor set that binding, whatever a caller sends.
    let mut body = api_call_body("get-item-b", "svc-b", "/items/1", &[]);
    body["auth_provider"] = json!("whatever");
    let created = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(3, "api_call.create", body),
    )
    .await;
    assert!(!is_error(&created), "got {created:?}");

    let fetched = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(4, "api_call.get", json!({"slug": "get-item-b"})),
    )
    .await;
    let fetched_json = tool_json(&fetched);
    assert_eq!(
        fetched_json["auth_provider"],
        Value::Null,
        "an auth_provider sent through this tool must never be honored: {fetched_json:?}"
    );

    h.db.teardown().await
}

/// The consequence of hiding auth providers, handled deliberately rather than left to happen by
/// accident: an api_call created over `/mcp` has no credential, so calling it sends none, and an
/// upstream `401` is just a normal recorded failure — not a crash, not a special "denied" status,
/// and no `Authorization` header ever leaves this process.
#[tokio::test]
async fn an_api_call_with_no_auth_provider_sends_no_credential_and_records_a_normal_401()
-> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    h.fixture.set("/secret", Behavior::Status(401));

    rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            1,
            "service.create",
            service_body("svc-c", &h.fixture.base_url()),
        ),
    )
    .await;
    rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            2,
            "api_call.create",
            api_call_body("get-secret", "svc-c", "/secret", &[]),
        ),
    )
    .await;
    rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(3, "endpoint.create", endpoint_body("ep-c", "not has(x)")),
    )
    .await;

    let tested = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            4,
            "api_call.test",
            json!({"slug": "get-secret", "endpoint": "ep-c", "args": {}}),
        ),
    )
    .await;
    let result = tool_json(&tested);
    assert_eq!(result["status"], json!("error"), "got {result:?}");
    assert!(
        result["error"].as_str().is_some_and(|e| e.contains("401")),
        "expected the upstream's own 401 to surface: {result:?}"
    );

    assert!(
        h.fixture
            .seen()
            .iter()
            .all(|r| !r.headers.contains_key("authorization")),
        "an api_call with no auth_provider must never send an Authorization header: {:?}",
        h.fixture.seen()
    );

    h.db.teardown().await
}

#[tokio::test]
async fn definitions_belong_to_their_owner_and_are_invisible_to_another_owner() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    h.fixture.set(
        "/items/1",
        Behavior::Json(json!({"id": "1", "title": "One"})),
    );
    rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(
            1,
            "service.create",
            service_body("owned-only", &h.fixture.base_url()),
        ),
    )
    .await;

    let other_user = h
        .stores
        .user()
        .create(NewUser {
            email: "other-control-owner@example.com".to_owned(),
            password: "correct horse battery staple".to_owned(),
        })
        .await?;
    let other_token = h
        .stores
        .service_token()
        .mint(
            other_user.id,
            "other owner's agent".to_owned(),
            None,
            BTreeSet::new(),
            true,
        )
        .await?;

    let list = rpc(
        &h.router,
        &other_token.plaintext,
        "/mcp",
        call_tool(1, "service.list", json!({})),
    )
    .await;
    assert_eq!(
        tool_json(&list),
        json!([]),
        "another owner must never see this owner's services"
    );

    let get = rpc(
        &h.router,
        &other_token.plaintext,
        "/mcp",
        call_tool(2, "service.get", json!({"slug": "owned-only"})),
    )
    .await;
    assert!(
        is_error(&get),
        "another owner must get a not-found business error, not the real row: {get:?}"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn an_invalid_definition_is_rejected_with_the_full_validation_error_list() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };

    let mut body = service_body("bad-svc", &h.fixture.base_url());
    // The service's own base_url origin is not in its own (now-empty) allowlist — the same
    // inconsistency `server::api::validate_write`'s own test pins for `/api`.
    body["origin_allowlist"] = json!([]);

    let resp = rpc(
        &h.router,
        &h.control_token,
        "/mcp",
        call_tool(1, "service.create", body),
    )
    .await;
    assert!(is_error(&resp), "got {resp:?}");
    let payload = tool_json(&resp);
    let errors = payload["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("allowlist")),
        "got {errors:?}"
    );

    h.db.teardown().await
}
