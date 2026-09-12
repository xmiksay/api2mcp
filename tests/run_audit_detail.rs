//! Gaps 2 and 3 of the audit-trail fixes: `execution_start` is persisted and returned by the
//! detail route, `GET /api/runs/{id}` exposes the full audit record (the list route still doesn't
//! carry the large `definition_snapshot`), and none of that newly-exposed surface can ever leak a
//! credential (I4) — everything here is already redacted at write time, so this is the read-side
//! proof of that, not a second redaction pass.

mod api_support;
mod common;
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use axum::http::{Method, StatusCode};
use serde_json::json;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, Service, Slug, Tag, TagExpr,
};
use api2mcp::store::Stores;

use api_support::{admin, setup};
use fixture::{Behavior, Fixture};

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

async fn seed_service(
    stores: &Stores,
    owner_id: uuid::Uuid,
    name: &str,
    fixture: &Fixture,
) -> Result<Slug> {
    let base_url = fixture.base_url();
    let service = Service {
        owner_id,
        slug: slug(name),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
        default_headers: BTreeMap::new(),
        timeout_ms: 5_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    };
    stores.service().create(&service).await?;
    Ok(service.slug)
}

fn plain_get_call(
    owner_id: uuid::Uuid,
    slug_str: &str,
    service_slug: &Slug,
    path: &str,
) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug(slug_str),
        service_slug: service_slug.clone(),
        auth_provider_slug: None,
        method: http::Method::GET,
        path_template: path.to_owned(),
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

#[tokio::test]
async fn execution_start_round_trips_and_the_detail_route_exposes_the_full_record() -> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/thing", Behavior::Json(json!({"n": 1})));

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-detail", &fixture).await?;
    let tag = Tag(slug("tag-detail"));
    h.stores
        .api_call()
        .create(
            &plain_get_call(h.admin_id, "thing", &service_slug, "/thing"),
            &BTreeSet::from([tag.clone()]),
        )
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-detail"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;

    let before = chrono::Utc::now();
    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/api_calls/thing/test",
        Some(json!({"args": {}, "endpoint": "ep-detail"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    let run_id = body["run_id"].as_str().expect("run_id present").to_owned();

    let (status, detail) = admin(&h, Method::GET, &format!("/api/runs/{run_id}"), None).await;
    assert_eq!(status, StatusCode::OK, "body: {detail:?}");

    // execution_start round-trips as a real, sane timestamp.
    let execution_start: chrono::DateTime<chrono::Utc> = detail["execution_start"]
        .as_str()
        .expect("execution_start present")
        .parse()
        .expect("execution_start is a valid RFC3339 timestamp");
    assert!(
        execution_start >= before,
        "execution_start must be captured no earlier than the request that triggered the run"
    );

    // The rest of what the detail route now exposes.
    assert_eq!(detail["definition_snapshot"]["kind"], json!("api_call"));
    assert!(detail["definition_digest"].as_str().is_some());
    assert!(detail["input_redacted"].is_object());
    assert_eq!(detail["output_redacted"], json!({"n": 1}));
    assert!(detail["errors"].is_null(), "no failure on this run");
    assert!(detail["budget_snapshot"].is_object());
    assert!(detail["timings"].is_object());
    assert_eq!(detail["caller_kind"], json!("service_token"));

    let calls = detail["calls"].as_array().expect("calls array");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["method"], json!("GET"));
    assert!(
        calls[0]["url_redacted"]
            .as_str()
            .unwrap()
            .ends_with("/thing")
    );
    assert!(calls[0]["headers_redacted"].is_object());
    assert_eq!(calls[0]["status_code"], json!(200));
    assert!(calls[0]["response_bytes"].as_u64().unwrap() > 0);

    // The list route still leaves the large snapshot off — deliberately, not an oversight.
    let (status, list) = admin(&h, Method::GET, "/api/runs?endpoint=ep-detail", None).await;
    assert_eq!(status, StatusCode::OK);
    let list = list.as_array().expect("list array");
    assert_eq!(list.len(), 1);
    assert!(
        list[0].get("definition_snapshot").is_none(),
        "the list route must never carry the snapshot: {list:?}"
    );
    // But the ordinary summary fields are still there.
    assert_eq!(list[0]["tool_name"], json!("thing"));

    h.db.teardown().await
}

#[tokio::test]
async fn a_credential_used_by_a_call_appears_in_neither_the_list_nor_the_detail_response()
-> Result<()> {
    let Some(h) = setup().await? else {
        return Ok(());
    };
    let fixture = Fixture::start().await;
    fixture.set("/secure", Behavior::Json(json!({"ok": true})));

    let service_slug = seed_service(&h.stores, h.admin_id, "svc-cred", &fixture).await?;
    let bound_origin = Origin::of(&fixture.base_url())?.to_string();

    let (status, resp) = admin(
        &h,
        Method::POST,
        "/api/auth_providers",
        Some(json!({
            "slug": "cred-provider",
            "service": service_slug.as_str(),
            "kind": "static_header",
            "credential_env_key": "A2M_TEST_RUN_AUDIT_CRED",
            "header_name": "X-Api-Key",
            "value_template": "",
            "bound_origin": bound_origin,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding auth provider: {resp:?}"
    );

    let mut call = plain_get_call(h.admin_id, "secure-thing", &service_slug, "/secure");
    call.auth_provider_slug = Some(slug("cred-provider"));
    let tag = Tag(slug("tag-cred"));
    h.stores
        .api_call()
        .create(&call, &BTreeSet::from([tag.clone()]))
        .await?;
    h.stores
        .endpoint()
        .create(&EndpointDef {
            owner_id: h.admin_id,
            slug: slug("ep-cred"),
            tag_expr: TagExpr::Has(tag),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        })
        .await?;

    const SECRET: &str = "sh-super-secret-run-audit-value";
    unsafe { std::env::set_var("A2M_TEST_RUN_AUDIT_CRED", SECRET) };

    let (status, body) = admin(
        &h,
        Method::POST,
        "/api/api_calls/secure-thing/test",
        Some(json!({"args": {}, "endpoint": "ep-cred"})),
    )
    .await;
    unsafe { std::env::remove_var("A2M_TEST_RUN_AUDIT_CRED") };
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(
        body["status"],
        json!("ok"),
        "the credentialed call must actually succeed"
    );
    let run_id = body["run_id"].as_str().expect("run_id present").to_owned();

    // The upstream actually received the header — proving the credential was genuinely used, not
    // just configured and never applied.
    let seen = fixture.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].headers.get("x-api-key").map(String::as_str),
        Some(SECRET)
    );

    let (status, detail) = admin(&h, Method::GET, &format!("/api/runs/{run_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, list) = admin(&h, Method::GET, "/api/runs?endpoint=ep-cred", None).await;
    assert_eq!(status, StatusCode::OK);

    let detail_text = serde_json::to_string(&detail)?;
    let list_text = serde_json::to_string(&list)?;
    assert!(
        !detail_text.contains(SECRET),
        "detail response: {detail_text}"
    );
    assert!(!list_text.contains(SECRET), "list response: {list_text}");

    h.db.teardown().await
}
