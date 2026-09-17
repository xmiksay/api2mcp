//! Rows to a validated [`plan::EndpointPlan`]. The invariant chokepoint.
//!
//! [`build_plan`] loads one endpoint's definitions, evaluates its tag expression over every
//! candidate api_call/script, compiles everything that can be compiled once (path templates,
//! projections, JSON Schemas), folds budgets, binds auth, and computes the reachable-origin
//! set — every statically checkable invariant this crate has (I1's data structure, I2, I3's
//! cross-check, I5, I6's static half), decided here, once, before an executor can exist. Any
//! failure fails the whole plan: a half-valid endpoint must never serve.

mod auth_bind;
mod budgets;
mod cache;
mod compile;
mod digest;
mod origins;
pub mod plan;
pub mod tag_expr;
mod tools;

pub use cache::PlanCache;
pub use plan::EndpointPlan;

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::http::TemplateError;
use crate::model::{ScriptDef, Slug, eval_tag_expr};
use crate::store::Stores;

/// Everything that can make [`build_plan`] fail, each naming the definition responsible.
/// `Serialize` so this can reach `GET /api/endpoints/{slug}/plan` (a later chunk) verbatim
/// instead of a generic 500.
#[derive(Debug, Clone, PartialEq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ResolveError {
    #[error("endpoint {slug:?} does not exist")]
    EndpointNotFound { slug: String },

    /// Any store read failure encountered while assembling a plan. `StoreError` itself isn't
    /// `Clone`/`Serialize` (it deliberately never carries a raw `DbErr`), so this carries its
    /// rendered message instead — the original is already logged at `error` level by
    /// `store::db_err` before it gets here.
    #[error("{0}")]
    Store(String),

    #[error("api_call {api_call:?}'s path template: {source}")]
    Template {
        api_call: String,
        #[source]
        source: TemplateError,
    },

    #[error(
        "api_call {api_call:?}: path template placeholders {placeholders:?} do not match its `location = path` params {path_params:?}"
    )]
    PathParamMismatch {
        api_call: String,
        placeholders: BTreeSet<String>,
        path_params: BTreeSet<String>,
    },

    #[error("api_call {api_call:?} projection field {field:?}: {message}")]
    Projection {
        api_call: String,
        field: String,
        message: String,
    },

    #[error(
        "api_call {api_call:?} on service {service:?}: origin {origin} is not in that service's allowlist"
    )]
    OriginNotAllowed {
        api_call: String,
        service: String,
        origin: String,
    },

    #[error(
        "api_call {api_call:?}: auth provider {provider:?} is bound to {bound_origin}, not this api_call's origin {api_call_origin}"
    )]
    AuthOriginMismatch {
        api_call: String,
        provider: String,
        bound_origin: String,
        api_call_origin: String,
    },

    #[error(
        "api_call {api_call:?}: auth provider {provider:?} is outside this endpoint's auth_providers scope"
    )]
    AuthProviderOutOfScope { api_call: String, provider: String },

    #[error("api_call {api_call:?} has access=write, above this endpoint's read write_ceiling")]
    WriteCeilingViolation { api_call: String },

    #[error("tool name {name:?} is claimed by more than one selected api_call/script")]
    DuplicateToolName { name: String },

    #[error(
        "endpoint_aliases: more than one alias names the same target ({target:?}): {aliases:?}"
    )]
    AmbiguousAlias {
        target: String,
        aliases: Vec<String>,
    },
}

/// Loads endpoint `slug`'s definitions, validates every static invariant, and returns the
/// resulting immutable plan — or the first violation found, naming what failed. See the
/// module doc for the pipeline this runs.
///
/// Every store read below is scoped to `owner_id`: a user owns everything they create and can
/// use only their own, so a bare slug reference — the endpoint itself, its selected api_calls
/// and scripts, an alias target, an auth-provider scope entry — always resolves *within this one
/// owner*, never across into someone else's definitions of the same slug.
pub async fn build_plan(
    stores: &Stores,
    owner_id: Uuid,
    slug: &Slug,
) -> Result<EndpointPlan, ResolveError> {
    let endpoint = stores
        .endpoint()
        .get(owner_id, slug)
        .await
        .map_err(|e| ResolveError::Store(e.to_string()))?
        .ok_or_else(|| ResolveError::EndpointNotFound {
            slug: slug.as_str().to_owned(),
        })?;

    let all_api_calls = stores
        .api_call()
        .list_all(owner_id)
        .await
        .map_err(|e| ResolveError::Store(e.to_string()))?;
    let all_scripts = stores
        .script()
        .list_all(owner_id)
        .await
        .map_err(|e| ResolveError::Store(e.to_string()))?;

    let mut calls = BTreeMap::new();
    for tagged in &all_api_calls {
        if !eval_tag_expr(&endpoint.tag_expr, &tagged.tags) {
            continue;
        }
        let planned = compile::compile_api_call(stores, owner_id, &tagged.api_call).await?;
        calls.insert(tagged.api_call.slug.clone(), planned);
    }

    let mut scripts: BTreeMap<Slug, ScriptDef> = BTreeMap::new();
    let mut callable_by: BTreeMap<Slug, BTreeMap<String, Slug>> = BTreeMap::new();
    for tagged in &all_scripts {
        if !eval_tag_expr(&endpoint.tag_expr, &tagged.tags) {
            continue;
        }
        // I1's data structure: the intersection of what the script declared with what this
        // endpoint actually selected. An api_call a script declares but this endpoint doesn't
        // expose can never become reachable through it.
        let reachable: BTreeMap<String, Slug> = tagged
            .script
            .callable
            .iter()
            .filter(|(_, target)| calls.contains_key(*target))
            .map(|(alias, target)| (alias.clone(), target.clone()))
            .collect();
        callable_by.insert(tagged.script.slug.clone(), reachable);
        scripts.insert(tagged.script.slug.clone(), tagged.script.clone());
    }

    let origins = origins::reachable_origins(&calls)?;
    auth_bind::assert_bound(
        &stores.auth_provider(),
        owner_id,
        &calls,
        &endpoint.auth_providers,
    )
    .await?;
    budgets::assert_write_ceiling(&calls, endpoint.write_ceiling)?;

    let tools = tools::build_tools(&endpoint.aliases, &endpoint.budgets, &calls, &scripts)?;

    let digest = digest::compute(
        &endpoint.slug,
        endpoint.write_ceiling,
        endpoint.budgets,
        &calls,
        &scripts,
        &tools,
    );

    Ok(EndpointPlan {
        owner_id,
        slug: endpoint.slug,
        write_ceiling: endpoint.write_ceiling,
        instructions: endpoint.instructions,
        tools,
        calls,
        scripts,
        callable_by,
        origins,
        budgets: endpoint.budgets,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthKind, AuthProvider, EndpointDef, Origin, Pagination, Service, Tag};
    use crate::store::AuthProviderStore;
    use crate::store::test_support::ScratchDb;

    #[test]
    fn eval_tag_expr_is_reexported_correctly() {
        // Sanity check that this module's `use` of `eval_tag_expr` really is
        // `model::tag::eval` and not something shadowing it.
        let expr = crate::model::TagExpr::Has(Tag("read".parse().unwrap()));
        let tags = BTreeSet::from([Tag("read".parse().unwrap())]);
        assert!(eval_tag_expr(&expr, &tags));
    }

    /// I5, end to end through `build_plan` (not just `auth_bind::assert_bound` in isolation).
    /// This has to live here rather than in `tests/resolve.rs`: setting up the fixture needs
    /// `AuthProviderStore::create`, which is `pub(crate)` (I5's structural enforcement — see
    /// `store::auth_provider`'s module doc) and therefore unreachable from an integration test,
    /// which compiles as a separate crate. `tests/resolve.rs` documents this same accommodation.
    #[tokio::test]
    async fn auth_provider_bound_to_a_foreign_origin_fails_the_plan_end_to_end() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();

        let base_url: url::Url = "https://svc-i5-e2e.example.com/".parse().unwrap();
        let service = Service {
            owner_id,
            slug: "svc-i5-e2e".parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        };
        stores.service().create(&service).await.unwrap();

        let provider = AuthProvider {
            owner_id,
            slug: "prov-e2e".parse().unwrap(),
            service_slug: service.slug.clone(),
            kind: AuthKind::StaticHeader,
            credential_env_key: "A2M_CRED_TEST_RESOLVE_E2E".into(),
            header_name: "Authorization".into(),
            value_template: "Bearer {token}".into(),
            scopes: vec![],
            token_url: None,
            // Bound to a different origin than the service it's attached to actually serves.
            bound_origin: "https://elsewhere.example.com".parse().unwrap(),
        };
        AuthProviderStore::new(db.db.clone())
            .create(&provider)
            .await
            .unwrap();

        let call = crate::model::ApiCall {
            owner_id,
            slug: "call-i5-e2e".parse().unwrap(),
            service_slug: service.slug.clone(),
            auth_provider_slug: Some(provider.slug.clone()),
            method: http::Method::GET,
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: crate::model::Access::Read,
            idempotent: true,
            projection: None,
            pagination: Pagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![],
            description: None,
        };
        stores
            .api_call()
            .create(&call, &BTreeSet::from([Tag("expose".parse().unwrap())]))
            .await
            .unwrap();

        let ep = EndpointDef {
            owner_id,
            slug: "ep-i5-e2e".parse().unwrap(),
            tag_expr: crate::model::TagExpr::Has(Tag("expose".parse().unwrap())),
            write_ceiling: crate::model::Access::Read,
            budgets: crate::model::Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        };
        stores.endpoint().create(&ep).await.unwrap();

        let err = build_plan(&stores, owner_id, &ep.slug).await.unwrap_err();
        assert!(matches!(err, ResolveError::AuthOriginMismatch { .. }));

        db.teardown().await.unwrap();
    }
}
