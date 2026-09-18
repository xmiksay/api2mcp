//! I5: the auth-provider-to-origin binding is set by a human, not proposed or changed by an
//! agent. This module re-asserts that binding at plan-build time, the last point before an
//! executor could otherwise trust it implicitly.
//!
//! A service has at most one auth provider (`ux_auth_providers_service`), and every api_call on
//! that service uses it, so this checks the binding **once per selected service** rather than
//! once per api_call — every api_call sharing a service also shares its origin (I2:
//! `PlannedApiCall::origin` is always `Origin::of(&service.base_url)`), so there is nothing a
//! second check on a sibling api_call could catch that the first didn't already.
//!
//! Two checks, per service with a provider that at least one selected api_call targets:
//! 1. `provider.bound_origin` matches the service's own origin — a provider bound to one origin
//!    can never be attached to a service that sends its credential somewhere else.
//! 2. The provider is within the endpoint's auth scope: an empty
//!    `EndpointDef::auth_providers` means "every provider belonging to a selected service" —
//!    and a non-empty one restricts to exactly the listed providers.
//!
//! A service with no provider at all is not a failure — its api_calls simply send no credential
//! (a freshly imported pack looks exactly like this until a human wires one up).
//!
//! Either failure fails the whole plan: a half-valid endpoint must never serve.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::model::Slug;
use crate::store::AuthProviderStore;

use super::ResolveError;
use super::plan::PlannedApiCall;

pub async fn assert_bound(
    auth_providers: &AuthProviderStore,
    owner_id: Uuid,
    calls: &BTreeMap<Slug, PlannedApiCall>,
    endpoint_scope: &BTreeSet<Slug>,
) -> Result<(), ResolveError> {
    // One representative planned call per service (the first encountered in `calls`'s own
    // BTreeMap order, i.e. alphabetically-first api_call slug on that service) — deterministic,
    // and every field this function reads off it (`service`, `origin`) is identical across every
    // api_call sharing that service anyway.
    let mut representative: BTreeMap<&Slug, &PlannedApiCall> = BTreeMap::new();
    for planned in calls.values() {
        representative
            .entry(&planned.service.slug)
            .or_insert(planned);
    }

    for planned in representative.values() {
        let Some(provider) = auth_providers
            .get_for_service(owner_id, &planned.service.slug)
            .await
            .map_err(|e| ResolveError::Store(e.to_string()))?
        else {
            continue;
        };

        if provider.bound_origin != planned.origin {
            return Err(ResolveError::AuthOriginMismatch {
                api_call: planned.api_call.slug.as_str().to_owned(),
                provider: provider.slug.as_str().to_owned(),
                bound_origin: provider.bound_origin.to_string(),
                api_call_origin: planned.origin.to_string(),
            });
        }

        if !endpoint_scope.is_empty() && !endpoint_scope.contains(&provider.slug) {
            return Err(ResolveError::AuthProviderOutOfScope {
                api_call: planned.api_call.slug.as_str().to_owned(),
                provider: provider.slug.as_str().to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Access, ApiCall, AuthKind, AuthProvider, CredentialSource, Origin, Pagination, Service,
    };
    use crate::store::test_support::ScratchDb;

    fn service(owner_id: Uuid, slug: &str) -> Service {
        let base_url: url::Url = format!("https://{slug}.example.com/").parse().unwrap();
        Service {
            owner_id,
            slug: slug.parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn api_call(owner_id: Uuid, service_slug: &Slug, slug: &str) -> ApiCall {
        ApiCall {
            owner_id,
            slug: slug.parse().unwrap(),
            service_slug: service_slug.clone(),
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
            params: vec![],
            description: None,
        }
    }

    fn provider(
        owner_id: Uuid,
        service_slug: &Slug,
        slug: &str,
        bound_origin: &str,
    ) -> AuthProvider {
        AuthProvider {
            owner_id,
            slug: slug.parse().unwrap(),
            service_slug: service_slug.clone(),
            kind: AuthKind::StaticHeader,
            credential: CredentialSource::Env("A2M_CRED_TEST_AUTH_BIND".into()),
            header_name: "Authorization".into(),
            value_template: "Bearer {token}".into(),
            scopes: vec![],
            token_url: None,
            bound_origin: bound_origin.parse().unwrap(),
        }
    }

    fn planned_call(service: Service, call: ApiCall) -> PlannedApiCall {
        let origin = Origin::of(&service.base_url).unwrap();
        PlannedApiCall {
            url_template: crate::http::UrlTemplate::parse(&call.path_template).unwrap(),
            api_call: call,
            service,
            origin,
            projection: None,
        }
    }

    #[tokio::test]
    async fn mismatched_bound_origin_fails_the_plan() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };

        let stores = crate::store::Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let svc = service(owner_id, "auth-bind-mismatch");
        stores.service().create(&svc).await.unwrap();
        let providers = AuthProviderStore::new(db.db.clone());
        // Bound to a *different* origin than the service the api_call actually targets.
        let p = provider(owner_id, &svc.slug, "prov", "https://elsewhere.example.com");
        providers.create(&p).await.unwrap();

        let call = api_call(owner_id, &svc.slug, "call-a");
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned_call(svc, call));

        let err = assert_bound(&providers, owner_id, &calls, &BTreeSet::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ResolveError::AuthOriginMismatch { .. }));

        db.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn provider_outside_a_non_empty_endpoint_scope_fails_the_plan() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };

        let stores = crate::store::Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let svc = service(owner_id, "auth-bind-scope");
        stores.service().create(&svc).await.unwrap();
        let providers = AuthProviderStore::new(db.db.clone());
        let p = provider(
            owner_id,
            &svc.slug,
            "prov",
            &Origin::of(&svc.base_url).unwrap().to_string(),
        );
        providers.create(&p).await.unwrap();

        let call = api_call(owner_id, &svc.slug, "call-a");
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned_call(svc, call));

        // Scope names a provider that isn't "prov".
        let scope = BTreeSet::from(["other-provider".parse().unwrap()]);
        let err = assert_bound(&providers, owner_id, &calls, &scope)
            .await
            .unwrap_err();
        assert!(matches!(err, ResolveError::AuthProviderOutOfScope { .. }));

        // An empty scope, by contrast, passes.
        assert!(
            assert_bound(&providers, owner_id, &calls, &BTreeSet::new())
                .await
                .is_ok()
        );

        db.teardown().await.unwrap();
    }

    /// A service with no provider at all is not a failure — its api_calls simply send no
    /// credential (see this module's own doc).
    #[tokio::test]
    async fn a_provider_less_service_passes() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };

        let stores = crate::store::Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let svc = service(owner_id, "auth-bind-no-provider");
        stores.service().create(&svc).await.unwrap();
        let providers = AuthProviderStore::new(db.db.clone());

        let call = api_call(owner_id, &svc.slug, "call-a");
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned_call(svc, call));

        assert!(
            assert_bound(&providers, owner_id, &calls, &BTreeSet::new())
                .await
                .is_ok()
        );

        db.teardown().await.unwrap();
    }

    /// Change 1's headline behavior: two api_calls on the same service share its one provider
    /// with no per-call wiring — the origin/scope check runs once (per service), not once per
    /// api_call, and both calls pass or fail together.
    #[tokio::test]
    async fn two_api_calls_on_one_service_share_its_provider() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };

        let stores = crate::store::Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let svc = service(owner_id, "auth-bind-shared");
        stores.service().create(&svc).await.unwrap();
        let providers = AuthProviderStore::new(db.db.clone());
        let p = provider(
            owner_id,
            &svc.slug,
            "prov",
            &Origin::of(&svc.base_url).unwrap().to_string(),
        );
        providers.create(&p).await.unwrap();

        let call_a = api_call(owner_id, &svc.slug, "call-a");
        let call_b = api_call(owner_id, &svc.slug, "call-b");
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned_call(svc.clone(), call_a));
        calls.insert("call-b".parse().unwrap(), planned_call(svc, call_b));

        // Neither api_call names a provider of its own — both pass purely because their shared
        // service has one whose origin matches.
        assert!(
            assert_bound(&providers, owner_id, &calls, &BTreeSet::new())
                .await
                .is_ok()
        );

        db.teardown().await.unwrap();
    }
}
