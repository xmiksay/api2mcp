//! Walks one endpoint's tag selection into a self-contained [`Pack`]: the transitive closure of
//! selected api_calls and scripts, the api_calls those scripts declare, and the services and
//! auth providers all of them need. A pack that imports into a plan-resolvable state is the
//! whole point (see [`super::import`]'s highest-value test), so a missing transitive dependency
//! here is a bug, not a warning — every branch below that could silently drop a reference
//! instead returns [`ExportError`].

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{AuthProvider, EndpointTarget, Slug, eval_tag_expr};
use crate::store::{StoreError, Stores};

use super::Pack;
use super::convert;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("endpoint {0:?} does not exist")]
    EndpointNotFound(String),
    #[error("{0}")]
    Store(String),
    #[error("script {script:?} declares api_call {api_call:?}, which does not exist")]
    MissingApiCall { script: String, api_call: String },
    #[error(
        "endpoint {endpoint:?}'s auth_providers scope names {provider:?}, which does not exist"
    )]
    MissingAuthProvider { endpoint: String, provider: String },
}

fn store_err(e: StoreError) -> ExportError {
    ExportError::Store(e.to_string())
}

/// Exports `slug`'s endpoint and everything it — directly or through a selected script —
/// depends on.
pub async fn export_endpoint(stores: &Stores, slug: &Slug) -> Result<Pack, ExportError> {
    let endpoint = stores
        .endpoint()
        .get(slug)
        .await
        .map_err(store_err)?
        .ok_or_else(|| ExportError::EndpointNotFound(slug.as_str().to_owned()))?;

    let calls_by_slug: BTreeMap<Slug, _> = stores
        .api_call()
        .list_all()
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|c| (c.api_call.slug.clone(), c))
        .collect();
    let scripts_by_slug: BTreeMap<Slug, _> = stores
        .script()
        .list_all()
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|s| (s.script.slug.clone(), s))
        .collect();
    let all_services = stores.service().list().await.map_err(store_err)?;

    // A global slug -> provider lookup. `EndpointDef::auth_providers` and an api_call's own
    // `auth_provider_slug` both name a bare (service-less) slug, and `auth_providers.slug` is
    // unique globally (see `store::auth_provider::id_by_slug_global`'s doc), so one scan over
    // every service covers every provider this pack could possibly need.
    let mut providers_by_slug: BTreeMap<Slug, AuthProvider> = BTreeMap::new();
    for svc in &all_services {
        for p in stores
            .auth_provider()
            .list_for_service(&svc.slug)
            .await
            .map_err(store_err)?
        {
            providers_by_slug.insert(p.slug.clone(), p);
        }
    }

    let mut selected_calls: BTreeSet<Slug> = calls_by_slug
        .iter()
        .filter(|(_, c)| eval_tag_expr(&endpoint.tag_expr, &c.tags))
        .map(|(s, _)| s.clone())
        .collect();
    let mut selected_scripts: BTreeSet<Slug> = scripts_by_slug
        .iter()
        .filter(|(_, s)| eval_tag_expr(&endpoint.tag_expr, &s.tags))
        .map(|(s, _)| s.clone())
        .collect();

    // An alias renames whatever it names regardless of whether the tag expression would have
    // selected it on its own, so its target must travel with the pack too.
    for target in endpoint.aliases.values() {
        match target {
            EndpointTarget::ApiCall(s) => {
                selected_calls.insert(s.clone());
            }
            EndpointTarget::Script(s) => {
                selected_scripts.insert(s.clone());
            }
        }
    }

    // I1's declarative half, from the pack's point of view: every api_call a selected script
    // declares must travel with it, checked eagerly so a dangling reference fails export instead
    // of silently producing a pack whose script can never call anything once imported.
    let mut needed_calls = selected_calls.clone();
    for slug in &selected_scripts {
        let Some(tagged) = scripts_by_slug.get(slug) else {
            continue;
        };
        for target in tagged.script.callable.values() {
            if !calls_by_slug.contains_key(target) {
                return Err(ExportError::MissingApiCall {
                    script: tagged.script.slug.as_str().to_owned(),
                    api_call: target.as_str().to_owned(),
                });
            }
            needed_calls.insert(target.clone());
        }
    }

    let mut api_calls = BTreeMap::new();
    let mut needed_services: BTreeSet<Slug> = BTreeSet::new();
    let mut needed_providers: BTreeSet<Slug> = BTreeSet::new();
    for slug in &needed_calls {
        // Existence is already guaranteed here: a directly tag-selected slug is a key of
        // `calls_by_slug` by construction, and a script-declared one was just checked above.
        let Some(tagged) = calls_by_slug.get(slug) else {
            continue;
        };
        needed_services.insert(tagged.api_call.service_slug.clone());
        if let Some(p) = &tagged.api_call.auth_provider_slug {
            needed_providers.insert(p.clone());
        }
        api_calls.insert(
            slug.as_str().to_owned(),
            convert::api_call_to_pack(&tagged.api_call, &tagged.tags),
        );
    }

    for slug in &endpoint.auth_providers {
        if !providers_by_slug.contains_key(slug) {
            return Err(ExportError::MissingAuthProvider {
                endpoint: endpoint.slug.as_str().to_owned(),
                provider: slug.as_str().to_owned(),
            });
        }
        needed_providers.insert(slug.clone());
    }

    let mut auth_providers = BTreeMap::new();
    for slug in &needed_providers {
        let Some(p) = providers_by_slug.get(slug) else {
            continue;
        };
        needed_services.insert(p.service_slug.clone());
        auth_providers.insert(slug.as_str().to_owned(), convert::auth_provider_to_pack(p));
    }

    let mut services = BTreeMap::new();
    for slug in &needed_services {
        if let Some(svc) = all_services.iter().find(|s| &s.slug == slug) {
            services.insert(slug.as_str().to_owned(), convert::service_to_pack(svc));
        }
    }

    let mut scripts = BTreeMap::new();
    for slug in &selected_scripts {
        if let Some(tagged) = scripts_by_slug.get(slug) {
            scripts.insert(
                slug.as_str().to_owned(),
                convert::script_to_pack(&tagged.script, &tagged.tags),
            );
        }
    }

    let mut tags: BTreeSet<String> = BTreeSet::new();
    for call in api_calls.values() {
        tags.extend(call.tags.iter().cloned());
    }
    for script in scripts.values() {
        tags.extend(script.tags.iter().cloned());
    }

    let endpoints = BTreeMap::from([(
        endpoint.slug.as_str().to_owned(),
        convert::endpoint_to_pack(&endpoint),
    )]);

    Ok(Pack {
        version: super::PACK_VERSION,
        services,
        auth_providers,
        api_calls,
        scripts,
        endpoints,
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Access, ApiCall, Origin, Pagination, Service, Tag};
    use crate::store::test_support::ScratchDb;

    fn slug(s: &str) -> Slug {
        s.parse().expect("valid slug")
    }

    // `ExportError::MissingApiCall`/`MissingAuthProvider` have no test exercising them: both
    // `script_api_calls.api_call_id` and `endpoint_auth_providers.auth_provider_id` are real
    // foreign keys with `ON DELETE CASCADE` (`m0004_scripts_tags.rs`, `m0005_endpoints.rs`), so
    // deleting an api_call or auth_provider also deletes the join row that named it — a script's
    // `callable`/an endpoint's `auth_providers` scope can never observe a dangling reference
    // through the store's own read path. Confirmed by trying exactly that scenario (create,
    // reference, delete) during development: the join row disappeared with it, and the "missing"
    // branch never ran. Both branches stay as defence-in-depth against a future schema change
    // that weakens or removes the cascade, not because they're reachable today.

    #[tokio::test]
    async fn selects_calls_by_tag_and_pulls_in_the_service() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());

        let base_url: url::Url = "https://svc-export-basic.example.com/".parse().unwrap();
        let service = Service {
            slug: slug("svc-export-basic"),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        };
        stores.service().create(&service).await.unwrap();

        let call = ApiCall {
            slug: slug("call-export-basic"),
            service_slug: service.slug.clone(),
            auth_provider_slug: None,
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
            description: Some("Fetch every thing.".to_owned()),
        };
        stores
            .api_call()
            .create(&call, &BTreeSet::from([Tag(slug("expose"))]))
            .await
            .unwrap();

        let endpoint = crate::model::EndpointDef {
            slug: slug("ep-export-basic"),
            tag_expr: crate::model::TagExpr::Has(Tag(slug("expose"))),
            write_ceiling: Access::Read,
            budgets: crate::model::Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::new(),
            auth_providers: BTreeSet::new(),
        };
        stores.endpoint().create(&endpoint).await.unwrap();

        let pack = export_endpoint(&stores, &endpoint.slug).await.unwrap();
        assert!(pack.api_calls.contains_key("call-export-basic"));
        assert!(pack.services.contains_key("svc-export-basic"));
        assert!(pack.endpoints.contains_key("ep-export-basic"));
        assert_eq!(pack.tags, BTreeSet::from(["expose".to_owned()]));

        db.teardown().await.unwrap();
    }
}
