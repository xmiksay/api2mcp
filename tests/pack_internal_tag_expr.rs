//! `internal_tag_expr` (#7) through the YAML pack: export carries the expression *and* every
//! api_call it selects (even one no exported script declares), import into a separate owner
//! reproduces the same plan, and a pack written before the field existed still imports.
//! Skipped when `TEST_DATABASE_URL` is unset.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use common::ScratchDb;

use api2mcp::model::{
    Access, ApiCall, Budgets, EndpointDef, Origin, Pagination, ScriptDef, Service, Slug, Tag,
    TagExpr,
};
use api2mcp::pack::{self, Pack};
use api2mcp::resolve::build_plan;
use api2mcp::store::Stores;
use uuid::Uuid;

fn slug(s: &str) -> Slug {
    s.parse().expect("valid slug")
}

fn tags(t: &str) -> BTreeSet<Tag> {
    BTreeSet::from([Tag(slug(t))])
}

fn api_call(owner_id: Uuid, name: &str) -> ApiCall {
    ApiCall {
        owner_id,
        slug: slug(name),
        service_slug: slug("svc-pack-internal"),
        method: http::Method::GET,
        path_template: "/things".to_owned(),
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
    }
}

/// A script tagged `public` calling `call-used`; `call-used` and `call-spare` are both tagged
/// `raw` (internal), and `call-spare` is declared by no script at all.
async fn seed(stores: &Stores, owner_id: Uuid) -> Result<Slug> {
    let base_url: url::Url = "https://svc-pack-internal.example.com/".parse()?;
    stores
        .service()
        .create(&Service {
            owner_id,
            slug: slug("svc-pack-internal"),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url)?]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        })
        .await?;
    for name in ["call-used", "call-spare"] {
        stores
            .api_call()
            .create(&api_call(owner_id, name), &tags("raw"))
            .await?;
    }
    stores
        .script()
        .create(
            &ScriptDef {
                owner_id,
                slug: slug("script-pack-internal"),
                source: "api(\"used\", #{})".to_owned(),
                params: Vec::new(),
                callable: BTreeMap::from([("used".to_owned(), slug("call-used"))]),
                budgets: Budgets::default(),
                description: None,
            },
            &tags("public"),
        )
        .await?;
    let ep = EndpointDef {
        owner_id,
        slug: slug("ep-pack-internal"),
        tag_expr: TagExpr::Has(Tag(slug("public"))),
        internal_tag_expr: Some(TagExpr::Has(Tag(slug("raw")))),
        write_ceiling: Access::Read,
        budgets: Budgets::default(),
        instructions: None,
        enabled: true,
        aliases: BTreeMap::new(),
        auth_providers: BTreeSet::new(),
    };
    stores.endpoint().create(&ep).await?;
    Ok(ep.slug)
}

#[tokio::test]
async fn export_and_reimport_preserve_internal_selection() -> Result<()> {
    let Some(db) = ScratchDb::create().await? else {
        eprintln!("TEST_DATABASE_URL unset — skipping");
        return Ok(());
    };
    db.migrate_up().await?;
    let stores = Stores::new(db.conn.clone());
    let source = db.create_user().await?;
    let target = db.create_user().await?;
    let ep_slug = seed(&stores, source).await?;

    let exported = pack::export_endpoint(&stores, source, &ep_slug).await?;
    let endpoint = &exported.endpoints["ep-pack-internal"];
    assert_eq!(endpoint.internal_tag_expr.as_deref(), Some("has(raw)"));
    assert!(exported.api_calls.contains_key("call-spare"));
    assert!(exported.api_calls.contains_key("call-used"));

    // Through YAML text, the way a pack actually travels.
    let yaml = serde_norway::to_string(&exported)?;
    let reparsed: Pack = serde_norway::from_str(&yaml)?;
    pack::validate(&reparsed).expect("exported pack validates");
    pack::import(&stores, &reparsed, false, target).await?;

    let source_plan = build_plan(&stores, source, &ep_slug).await?;
    let target_plan = build_plan(&stores, target, &ep_slug).await?;
    assert_eq!(source_plan.digest, target_plan.digest);
    let internal: Vec<&str> = target_plan
        .internal_calls()
        .into_iter()
        .map(Slug::as_str)
        .collect();
    assert_eq!(internal, vec!["call-spare", "call-used"]);
    assert_eq!(
        target_plan
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["script-pack-internal"]
    );

    db.teardown().await
}

#[test]
fn a_pack_without_the_field_still_parses_and_round_trips_unchanged() {
    let yaml = "tag_expr: has(demo)\n";
    let endpoint: pack::PackEndpoint = serde_norway::from_str(yaml).expect("older endpoint shape");
    assert_eq!(endpoint.internal_tag_expr, None);
    let out = serde_norway::to_string(&endpoint).expect("serialize");
    assert!(
        !out.contains("internal_tag_expr"),
        "an unset field must not appear on export: {out}"
    );
}

#[test]
fn a_blank_internal_tag_expr_reads_as_none() {
    for blank in ["\"\"", "\"   \"", "null"] {
        let yaml = format!("tag_expr: has(demo)\ninternal_tag_expr: {blank}\n");
        let endpoint: pack::PackEndpoint = serde_norway::from_str(&yaml).expect("parses");
        assert_eq!(endpoint.internal_tag_expr, None, "{blank}");
    }
}

#[test]
fn validate_rejects_an_alias_of_an_internal_api_call() {
    let text = std::fs::read_to_string("examples/demo.pack.yaml").expect("demo pack");
    let mut demo: Pack = serde_norway::from_str(&text).expect("demo pack parses");
    let endpoint = demo.endpoints.get_mut("demo").expect("demo endpoint");
    // Every demo api_call is tagged `demo`, so this makes `get-item` internal.
    endpoint.internal_tag_expr = Some("has(demo)".to_owned());
    endpoint.aliases.insert(
        "fetch".to_owned(),
        pack::PackEndpointTarget::ApiCall("get-item".to_owned()),
    );
    let errors = pack::validate(&demo).expect_err("alias of an internal api_call");
    assert!(
        errors.iter().any(|e| e
            .to_string()
            .starts_with("endpoints.demo: alias \"fetch\" names api_call \"get-item\"")),
        "{errors:?}"
    );
}

#[test]
fn validate_rejects_a_malformed_internal_tag_expr() {
    let text = std::fs::read_to_string("examples/demo.pack.yaml").expect("demo pack");
    let mut demo: Pack = serde_norway::from_str(&text).expect("demo pack parses");
    demo.endpoints
        .get_mut("demo")
        .expect("demo endpoint")
        .internal_tag_expr = Some("has(".to_owned());
    let errors = pack::validate(&demo).expect_err("malformed expression");
    assert!(
        errors.iter().any(|e| e
            .to_string()
            .starts_with("endpoints.demo: internal_tag_expr:")),
        "{errors:?}"
    );
}
