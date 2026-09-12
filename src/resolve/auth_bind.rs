//! I5: the auth-provider-to-origin binding is set by a human, not proposed or changed by an
//! agent. This module re-asserts that binding at plan-build time, the last point before an
//! executor could otherwise trust it implicitly.
//!
//! Two checks, per selected api_call that declares an auth provider:
//! 1. `provider.bound_origin` matches `PlannedApiCall::origin` (the api_call's own,
//!    service-derived origin) — a provider bound to one origin can never be attached to an
//!    api_call that sends its credential somewhere else.
//! 2. The provider is within the endpoint's auth scope: an empty
//!    `EndpointDef::auth_providers` means "every provider belonging to a selected service" —
//!    which every api_call's provider already is, by construction
//!    (`store::api_call::resolve_foreign_keys` looks a provider up scoped to the api_call's
//!    own service, so a cross-service reference can't exist on a stored row) — and a
//!    non-empty one restricts to exactly the listed providers.
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
    for planned in calls.values() {
        let Some(provider_slug) = &planned.api_call.auth_provider_slug else {
            continue;
        };
        let provider = auth_providers
            .get(owner_id, &planned.api_call.service_slug, provider_slug)
            .await
            .map_err(|e| ResolveError::Store(e.to_string()))?
            .ok_or_else(|| {
                ResolveError::Store(format!(
                    "api_call {:?} names auth provider {:?}, which no longer exists on service {:?}",
                    planned.api_call.slug.as_str(),
                    provider_slug.as_str(),
                    planned.api_call.service_slug.as_str()
                ))
            })?;

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
    use crate::model::{Access, ApiCall, AuthKind, AuthProvider, Origin, Pagination, Service};
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

    fn api_call(
        owner_id: Uuid,
        service_slug: &Slug,
        slug: &str,
        provider: Option<&str>,
    ) -> ApiCall {
        ApiCall {
            owner_id,
            slug: slug.parse().unwrap(),
            service_slug: service_slug.clone(),
            auth_provider_slug: provider.map(|p| p.parse().unwrap()),
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
            credential_env_key: "A2M_CRED_TEST_AUTH_BIND".into(),
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

        let call = api_call(owner_id, &svc.slug, "call-a", Some("prov"));
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

        let call = api_call(owner_id, &svc.slug, "call-a", Some("prov"));
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
}
