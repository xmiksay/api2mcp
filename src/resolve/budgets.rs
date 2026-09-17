//! I6's static half: folding budgets and enforcing the endpoint's write ceiling. The dynamic
//! half — actually metering calls/bytes/wall-clock/pages against the folded result — is
//! `runtime::budget` (a later chunk); this module only ever produces the numbers that
//! meter reads, never enforces them itself.

use std::collections::BTreeMap;

use crate::model::{Access, Slug};

use super::ResolveError;
use super::plan::PlannedApiCall;

/// A `read`-ceiling endpoint must not select any `access = write` api_call — directly, or
/// through a script's declared calls. Checking every entry in `calls` already covers both:
/// `EndpointPlan::callable_by` is defined as an intersection with `calls`'s keys (I1), so an
/// api_call a script can actually reach is necessarily a member of `calls` too. An api_call a
/// script *declared* but this endpoint doesn't select never becomes reachable at all (I1) —
/// its `access` is therefore irrelevant to this endpoint's ceiling.
pub fn assert_write_ceiling(
    calls: &BTreeMap<Slug, PlannedApiCall>,
    write_ceiling: Access,
) -> Result<(), ResolveError> {
    for planned in calls.values() {
        if planned.api_call.access > write_ceiling {
            return Err(ResolveError::WriteCeilingViolation {
                api_call: planned.api_call.slug.as_str().to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::http::UrlTemplate;
    use crate::model::{ApiCall, Origin, Pagination, Service};

    fn planned(access: Access) -> PlannedApiCall {
        let base_url: url::Url = "https://acme.example.com/".parse().unwrap();
        let service = Service {
            owner_id: uuid::Uuid::nil(),
            slug: "acme".parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: [Origin::of(&base_url).unwrap()].into_iter().collect(),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        };
        let api_call = ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: "call-a".parse().unwrap(),
            service_slug: service.slug.clone(),
            auth_provider_slug: None,
            method: http::Method::GET,
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access,
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
            origin: Origin::of(&service.base_url).unwrap(),
            api_call,
            service,
            projection: None,
        }
    }

    #[test]
    fn read_ceiling_rejects_a_write_call() {
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(Access::Write));
        let err = assert_write_ceiling(&calls, Access::Read).unwrap_err();
        assert!(matches!(err, ResolveError::WriteCeilingViolation { .. }));
    }

    #[test]
    fn read_ceiling_accepts_a_read_call() {
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(Access::Read));
        assert!(assert_write_ceiling(&calls, Access::Read).is_ok());
    }

    #[test]
    fn write_ceiling_accepts_everything() {
        let mut calls = BTreeMap::new();
        calls.insert("call-a".parse().unwrap(), planned(Access::Write));
        assert!(assert_write_ceiling(&calls, Access::Write).is_ok());
    }
}
