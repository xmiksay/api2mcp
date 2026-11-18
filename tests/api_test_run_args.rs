//! Integration tests for the `args` field of `POST /api/api_calls/{slug}/test` and
//! `POST /api/scripts/{slug}/test`: a missing or `null` `args` means `{}` (as an omitted
//! `arguments` does on MCP `tools/call`), while a present non-object is still a binding error.
//! Split from `tests/api_test_run.rs` to keep both under the 400-line cap.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, ScriptDef, Service, Slug, Tag,
    TagExpr,
};

use api_support::{Harness, admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

/// One endpoint exposing a param-less api_call (`list-items`) and a param-less script
/// (`hello`) — the exact shape issue #10 was about.
async fn seed(h: &Harness, fixture: &Fixture) -> Result<()> {
    let base_url = fixture.base_url();
    let service = Service {
        owner_id: h.admin_id,
        slug: slug("svc-args"),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    h.stores.service().create(&service).await?;
    let tags = BTreeSet::from([Tag(slug("args"))]);
    h.stores
        .api_call()
        .create(
            &ApiCall {
                owner_id: h.admin_id,
                slug: slug("list-items"),
                service_slug: service.slug.clone(),
                method: http::Method::GET,
                path_template: "/items".to_owned(),
                query_fixed: BTreeMap::new(),
                body_template: None,
                access: Access::Read,
                idempotent: true,
                projection: None,
                pagination: Pagination::None,
                timeout_ms: None,
                max_response_bytes: None,
                params: Vec::new(),
                description: None,
            },
            &tags,
        )
        .await?;
    h.stores
        .script()
        .create(
            &ScriptDef {
                owner_id: h.admin_id,
                slug: slug("hello"),
                source: r#"#{ "hello": "world" }"#.to_owned(),
                params: Vec::new(),
                callable: BTreeMap::new(),
                budgets: Budgets::default(),
                description: None,
            },
            &tags,
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-args"),
            tag_expr: TagExpr::Has(Tag(slug("args"))),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;
    Ok(())
}

/// The request bodies under test: `args` absent, `null`, `{}`, and two non-objects.
fn bodies() -> [(&'static str, Value, bool); 5] {
    [
        ("absent", json!({"endpoint": "ep-args"}), true),
        ("null", json!({"endpoint": "ep-args", "args": null}), true),
        ("object", json!({"endpoint": "ep-args", "args": {}}), true),
        ("string", json!({"endpoint": "ep-args", "args": "x"}), false),
        ("array", json!({"endpoint": "ep-args", "args": []}), false),
    ]
}

#[tokio::test]
async fn api_call_test_defaults_a_missing_args_to_an_empty_object() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/items", Behavior::Json(json!({"items": []})));
    seed(&h, &fixture).await?;

    for (case, request, accepted) in bodies() {
        let (status, body) = admin(
            &h,
            Method::POST,
            "/api/api_calls/list-items/test",
            Some(request),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{case}: {body:?}");
        if accepted {
            assert_eq!(body["status"], json!("ok"), "{case}: {body:?}");
            assert_eq!(body["projected"], json!({"items": []}), "{case}: {body:?}");
        } else {
            assert_eq!(body["status"], json!("error"), "{case}: {body:?}");
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("must be a JSON object")),
                "{case}: {body:?}"
            );
        }
    }

    h.db.teardown().await
}

#[tokio::test]
async fn script_test_defaults_a_missing_args_to_an_empty_object() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    seed(&h, &fixture).await?;

    for (case, request, accepted) in bodies() {
        let (status, body) =
            admin(&h, Method::POST, "/api/scripts/hello/test", Some(request)).await;
        assert_eq!(status, StatusCode::OK, "{case}: {body:?}");
        if accepted {
            assert!(body["failure"].is_null(), "{case}: {body:?}");
            assert_eq!(body["value"], json!({"hello": "world"}), "{case}: {body:?}");
        } else {
            assert_eq!(body["status"], json!("error"), "{case}: {body:?}");
            assert!(
                body["failure"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("must be a JSON object")),
                "{case}: {body:?}"
            );
        }
    }

    h.db.teardown().await
}
