//! `api_call.test` / `script.test` over the `/mcp` control plane: raw upstream bodies are
//! omitted unless `include_raw: true`, while each call's `status_code`/`response_bytes` always
//! stay (issue #9). `run.get` — the natural follow-up — takes the same flag. The `/api` routes are covered in `tests/api_test_run.rs`.

mod common;
mod fixture;

#[path = "mcp_control_plane_support/harness.rs"]
mod cp;
#[path = "mcp_support/harness.rs"]
mod harness;

use anyhow::Result;
use serde_json::{Value, json};

use cp::{Cp, api_call_body, call_tool, endpoint_body, service_body, setup, tool_json};
use fixture::Behavior;
use harness::rpc;

async fn tool(h: &Cp, id: i64, name: &str, arguments: Value) -> Value {
    tool_json(
        &rpc(
            &h.router,
            &h.control_token,
            "/mcp",
            call_tool(id, name, arguments),
        )
        .await,
    )
}

async fn seed(h: &Cp) {
    h.fixture
        .set("/item", Behavior::Json(json!({"id": "1", "title": "One"})));
    tool(
        h,
        1,
        "service.create",
        service_body("svc", &h.fixture.base_url()),
    )
    .await;
    tool(
        h,
        2,
        "api_call.create",
        api_call_body("get-item", "svc", "/item", &["raw"]),
    )
    .await;
    tool(
        h,
        3,
        "script.create",
        json!({
            "slug": "wrap",
            "source": "api(\"get\", #{})",
            "callable": {"get": "get-item"},
            "tags": ["raw"]
        }),
    )
    .await;
    tool(h, 4, "endpoint.create", endpoint_body("ep", "has(raw)")).await;
}

#[tokio::test]
async fn api_call_test_omits_raw_unless_asked_but_always_reports_status_and_size() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await;

    let base = json!({"slug": "get-item", "endpoint": "ep", "args": {}});
    let r = tool(&h, 10, "api_call.test", base.clone()).await;
    assert_eq!(r["status"], json!("ok"), "{r:?}");
    assert!(
        r.get("raw").is_none(),
        "raw must be absent by default: {r:?}"
    );
    assert_eq!(r["status_code"], json!(200));
    assert!(r["response_bytes"].as_u64().is_some_and(|n| n > 0), "{r:?}");

    let mut with_null = base.clone();
    with_null["include_raw"] = Value::Null;
    let r = tool(&h, 11, "api_call.test", with_null).await;
    assert_eq!(
        r["status"],
        json!("ok"),
        "include_raw: null is accepted: {r:?}"
    );
    assert!(r.get("raw").is_none(), "null means false: {r:?}");

    let mut with_raw = base;
    with_raw["include_raw"] = json!(true);
    let r = tool(&h, 12, "api_call.test", with_raw).await;
    assert_eq!(r["raw"], json!({"id": "1", "title": "One"}), "{r:?}");
    assert_eq!(r["status_code"], json!(200));

    h.db.teardown().await
}

#[tokio::test]
async fn script_test_omits_raw_unless_asked_but_always_reports_status_and_size() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await;

    let base = json!({"slug": "wrap", "endpoint": "ep", "args": {}});
    let r = tool(&h, 10, "script.test", base.clone()).await;
    assert!(r["failure"].is_null(), "{r:?}");
    let call = &r["calls"][0];
    assert!(
        call.get("raw").is_none(),
        "raw must be absent by default: {r:?}"
    );
    assert_eq!(call["status_code"], json!(200));
    assert!(
        call["response_bytes"].as_u64().is_some_and(|n| n > 0),
        "{r:?}"
    );

    let mut with_null = base.clone();
    with_null["include_raw"] = Value::Null;
    let r = tool(&h, 11, "script.test", with_null).await;
    assert!(
        r["calls"][0].get("raw").is_none(),
        "null means false: {r:?}"
    );

    let mut with_raw = base;
    with_raw["include_raw"] = json!(true);
    let r = tool(&h, 12, "script.test", with_raw).await;
    assert_eq!(
        r["calls"][0]["raw"],
        json!({"id": "1", "title": "One"}),
        "{r:?}"
    );

    h.db.teardown().await
}

#[tokio::test]
async fn run_get_omits_raw_unless_asked_but_always_reports_status_and_size() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    seed(&h).await;

    let tested = tool(
        &h,
        10,
        "script.test",
        json!({"slug": "wrap", "endpoint": "ep", "args": {}}),
    )
    .await;
    let id = tested["run_id"].clone();

    for args in [json!({"id": id}), json!({"id": id, "include_raw": null})] {
        let r = tool(&h, 11, "run.get", args).await;
        let call = &r["calls"][0];
        assert!(call.get("raw").is_none(), "raw must be absent: {r:?}");
        assert_eq!(call["status_code"], json!(200));
        assert!(
            call["response_bytes"].as_u64().is_some_and(|n| n > 0),
            "{r:?}"
        );
    }

    let r = tool(&h, 12, "run.get", json!({"id": id, "include_raw": true})).await;
    assert_eq!(
        r["calls"][0]["raw"],
        json!({"id": "1", "title": "One"}),
        "{r:?}"
    );

    h.db.teardown().await
}
