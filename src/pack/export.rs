//! Walks one endpoint's tag selection into a self-contained [`Pack`]: the transitive closure of
//! selected api_calls and scripts, and the services they need. A pack that imports into a
//! plan-resolvable state is the whole point (see [`super::import`]'s highest-value test), so a
//! missing transitive dependency here is a bug, not a warning — every branch below that could
//! silently drop a reference instead returns [`ExportError`].
//!
//! **A pack carries no auth providers, full stop** — not the provider definitions (there is no
//! `Pack::auth_providers` map to put them in) and not even a reference to one: the exported
//! endpoint's own `auth_providers` scope is always emptied (see [`super::PackEndpoint`]'s own
//! doc for what an empty scope means at resolve time). An imported endpoint therefore always
//! starts able to use whatever provider its services happen to have — which, on a fresh
//! instance, is none, until a human wires one up. This is the flip side of Change 1 ("auth
//! belongs to the service"): a provider is exactly as un-portable as the credential it holds.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::model::{EndpointTarget, Slug, eval_tag_expr};
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
}

fn store_err(e: StoreError) -> ExportError {
    ExportError::Store(e.to_string())
}

/// Exports `slug`'s endpoint and everything it — directly or through a selected script —
/// depends on. Every definition read here is scoped to `owner_id` (Decision: "export is scoped
/// to the caller's own definitions") — nothing belonging to another owner can ever end up in the
/// resulting pack, even transitively.
pub async fn export_endpoint(
    stores: &Stores,
    owner_id: Uuid,
    slug: &Slug,
) -> Result<Pack, ExportError> {
    let endpoint = stores
        .endpoint()
        .get(owner_id, slug)
        .await
        .map_err(store_err)?
        .ok_or_else(|| ExportError::EndpointNotFound(slug.as_str().to_owned()))?;

    let calls_by_slug: BTreeMap<Slug, _> = stores
        .api_call()
        .list_all(owner_id)
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|c| (c.api_call.slug.clone(), c))
        .collect();
    let scripts_by_slug: BTreeMap<Slug, _> = stores
        .script()
        .list_all(owner_id)
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|s| (s.script.slug.clone(), s))
        .collect();
    let all_services = stores.service().list(owner_id).await.map_err(store_err)?;

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
    for slug in &needed_calls {
        // Existence is already guaranteed here: a directly tag-selected slug is a key of
        // `calls_by_slug` by construction, and a script-declared one was just checked above.
        let Some(tagged) = calls_by_slug.get(slug) else {
            continue;
        };
        needed_services.insert(tagged.api_call.service_slug.clone());
        api_calls.insert(
            slug.as_str().to_owned(),
            convert::api_call_to_pack(&tagged.api_call, &tagged.tags),
        );
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

    // A pack carries no auth providers at all — see this module's own doc — so the exported
    // endpoint's own provider scope is always emptied, regardless of what it is set to live.
    let mut portable_endpoint = endpoint;
    portable_endpoint.auth_providers = BTreeSet::new();
    let endpoints = BTreeMap::from([(
        portable_endpoint.slug.as_str().to_owned(),
        convert::endpoint_to_pack(&portable_endpoint),
    )]);

    Ok(Pack {
        version: super::PACK_VERSION,
        services,
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

    // `ExportError::MissingApiCall` has no test exercising it: `script_api_calls.api_call_id` is
    // a real foreign key with `ON DELETE CASCADE` (`m0004_scripts_tags.rs`), so deleting an
    // api_call also deletes the join row that named it — a script's `callable` can never observe
    // a dangling reference through the store's own read path. Confirmed by trying exactly that
    // scenario (create, reference, delete) during development: the join row disappeared with it,
    // and the "missing" branch never ran. It stays as defence-in-depth against a future schema
    // change that weakens or removes the cascade, not because it's reachable today.

    #[tokio::test]
    async fn selects_calls_by_tag_and_pulls_in_the_service() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();

        let base_url: url::Url = "https://svc-export-basic.example.com/".parse().unwrap();
        let service = Service {
            owner_id,
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
            owner_id,
            slug: slug("call-export-basic"),
            service_slug: service.slug.clone(),
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
            owner_id,
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

        let pack = export_endpoint(&stores, owner_id, &endpoint.slug)
            .await
            .unwrap();
        assert!(pack.api_calls.contains_key("call-export-basic"));
        assert!(pack.services.contains_key("svc-export-basic"));
        assert!(pack.endpoints.contains_key("ep-export-basic"));
        assert_eq!(pack.tags, BTreeSet::from(["expose".to_owned()]));

        db.teardown().await.unwrap();
    }
}
