//! I2: the set of origins an endpoint can reach is computable statically, before execution.
//!
//! An api_call's request is always assembled from its service's `base_url`
//! (`http::bind::assemble_url` never lets a param move the scheme/host/port — that's I3), so
//! `PlannedApiCall::origin` (computed once, in `super::build_plan`) is already exactly what
//! this module needs to check. The origin *allowlist* is a broader set: it also bounds where
//! a same-origin-checked redirect ([`crate::http::send`]) may land at *runtime*. What this module
//! checks is the narrower, static half: that a plan never selects an api_call whose own
//! service doesn't even allow its own `base_url`'s origin — the case design correction #7
//! expects publish-time validation to prevent, and this is the defence-in-depth backstop if
//! that validation is ever skipped or buggy.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Origin, Slug};

use super::ResolveError;
use super::plan::PlannedApiCall;

/// Computes the reachable-origin set for a plan's selected api_calls, failing the whole plan
/// (a half-valid endpoint must never serve) if any of them resolves to an origin outside its
/// own service's allowlist.
///
/// Takes the already-loaded `calls` map rather than a half-built [`super::EndpointPlan`]
/// (the plan doesn't exist yet at the point `build_plan` needs this) — every api_call
/// reachable through a selected script is already a member of this map, because
/// `EndpointPlan::callable_by` is defined as an intersection with it (I1), so walking just
/// this map already covers "endpoint's selected api_calls and scripts' declared api_calls"
/// in one pass.
pub fn reachable_origins(
    calls: &BTreeMap<Slug, PlannedApiCall>,
) -> Result<BTreeSet<Origin>, ResolveError> {
    let mut origins = BTreeSet::new();
    for planned in calls.values() {
        if !planned.service.origin_allowlist.contains(&planned.origin) {
            return Err(ResolveError::OriginNotAllowed {
                api_call: planned.api_call.slug.as_str().to_owned(),
                service: planned.service.slug.as_str().to_owned(),
                origin: planned.origin.to_string(),
            });
        }
        origins.insert(planned.origin.clone());
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::http::UrlTemplate;
    use crate::model::{Access, ApiCall, Pagination, Service};

    fn service(slug: &str, allow_self: bool) -> Service {
        let base_url: url::Url = format!("https://{slug}.example.com/").parse().unwrap();
        let mut allowlist = BTreeSet::new();
        if allow_self {
            allowlist.insert(Origin::of(&base_url).unwrap());
        }
        Service {
            owner_id: uuid::Uuid::nil(),
            slug: slug.parse().unwrap(),
            base_url,
            origin_allowlist: allowlist,
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn planned(service: Service, call_slug: &str) -> PlannedApiCall {
        let origin = Origin::of(&service.base_url).unwrap();
        let api_call = ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: call_slug.parse().unwrap(),
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
            params: vec![],
            description: None,
        };
        PlannedApiCall {
            url_template: UrlTemplate::parse(&api_call.path_template).unwrap(),
            api_call,
            service,
            origin,
            projection: None,
        }
    }

    #[test]
    fn allowed_origin_is_collected() {
        let svc = service("acme", true);
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(svc.clone(), "call-a"));

        let origins = reachable_origins(&calls).expect("within allowlist");
        assert_eq!(
            origins,
            BTreeSet::from([Origin::of(&svc.base_url).unwrap()])
        );
    }

    #[test]
    fn origin_outside_allowlist_fails_the_whole_plan() {
        let svc = service("acme", false);
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(svc, "call-a"));

        let err = reachable_origins(&calls).unwrap_err();
        assert!(matches!(err, ResolveError::OriginNotAllowed { .. }));
    }

    #[test]
    fn multiple_services_union_into_one_set() {
        let a = service("svc-a", true);
        let b = service("svc-b", true);
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(a.clone(), "call-a"));
        calls.insert("call-b".parse().unwrap(), planned(b.clone(), "call-b"));

        let origins = reachable_origins(&calls).unwrap();
        assert_eq!(
            origins,
            BTreeSet::from([
                Origin::of(&a.base_url).unwrap(),
                Origin::of(&b.base_url).unwrap()
            ])
        );
    }
}
