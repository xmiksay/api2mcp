//! Unit tests for `items.rs` — split out purely to keep that file under the workspace's
//! 400-line cap, mirroring `http/bind.rs`'s identical `#[path = "bind_tests.rs"]` split.

use std::collections::BTreeMap;

use super::super::tests::{empty_pack, service};
use super::*;
use crate::pack::{PackParam, PackParamLocation};

#[test]
fn reports_every_failure_at_once() {
    let mut pack = empty_pack();
    pack.services.insert("svc".to_owned(), service());
    // Two independent problems: an unknown service reference, and a script naming a
    // nonexistent api_call.
    pack.api_calls.insert(
        "call-a".to_owned(),
        PackApiCall {
            service: "does-not-exist".to_owned(),
            method: "GET".to_owned(),
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: "read".to_owned(),
            idempotent: true,
            projection: None,
            pagination: PackPagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: Vec::new(),
            tags: BTreeSet::new(),
            description: None,
        },
    );
    pack.scripts.insert(
        "script-a".to_owned(),
        PackScript {
            source: "()".to_owned(),
            params: Vec::new(),
            callable: BTreeMap::from([("a".to_owned(), "no-such-call".to_owned())]),
            budgets: Default::default(),
            description: None,
            tags: BTreeSet::new(),
        },
    );

    let errors = crate::pack::validate(&pack).unwrap_err();
    assert!(
        errors.len() >= 2,
        "expected at least two failures, got {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::ApiCall { .. }))
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::Script { .. }))
    );
}

#[test]
fn fixed_param_shape_matches_url_template() {
    let mut pack = empty_pack();
    pack.services.insert("svc".to_owned(), service());
    pack.api_calls.insert(
        "call-a".to_owned(),
        PackApiCall {
            service: "svc".to_owned(),
            method: "GET".to_owned(),
            path_template: "/items/{id}".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: "read".to_owned(),
            idempotent: true,
            projection: None,
            pagination: PackPagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            // Missing the `id` path param entirely.
            params: vec![PackParam {
                name: "format".to_owned(),
                location: PackParamLocation::Query,
                ty: "string".to_owned(),
                required: false,
                default: None,
                fixed: Some(serde_json::json!("json")),
                enum_values: None,
                description: None,
                position: 0,
            }],
            tags: BTreeSet::new(),
            description: None,
        },
    );
    let errors = crate::pack::validate(&pack).unwrap_err();
    assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ApiCall { message, .. } if message.contains("placeholders")))
        );
}

/// The publish-time half of I3's `HEADER_PARAM_ALLOWLIST` check: a disallowed header
/// param name (`Authorization`, chief among them) must fail `pack::validate`, not merely
/// `http::bind` at dispatch time — everything else in this system validates at publish
/// time precisely so a definition cannot be saved broken.
#[test]
fn a_header_param_named_authorization_is_rejected_at_validate_time() {
    let mut pack = empty_pack();
    pack.services.insert("svc".to_owned(), service());
    pack.api_calls.insert(
        "call-a".to_owned(),
        PackApiCall {
            service: "svc".to_owned(),
            method: "GET".to_owned(),
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: "read".to_owned(),
            idempotent: true,
            projection: None,
            pagination: PackPagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![PackParam {
                name: "Authorization".to_owned(),
                location: PackParamLocation::Header,
                ty: "string".to_owned(),
                required: false,
                default: None,
                fixed: None,
                enum_values: None,
                description: None,
                position: 0,
            }],
            tags: BTreeSet::new(),
            description: None,
        },
    );
    let errors = crate::pack::validate(&pack).unwrap_err();
    assert!(
            errors.iter().any(
                |e| matches!(e, ValidationError::ApiCall { message, .. } if message.contains("Authorization") && message.contains("allowlist"))
            ),
            "expected a header-allowlist failure naming Authorization, got {errors:?}"
        );
}

/// The mirror-image case: an allowlisted header name must not be rejected.
#[test]
fn a_header_param_using_an_allowed_name_passes() {
    let mut pack = empty_pack();
    pack.services.insert("svc".to_owned(), service());
    pack.api_calls.insert(
        "call-a".to_owned(),
        PackApiCall {
            service: "svc".to_owned(),
            method: "GET".to_owned(),
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: "read".to_owned(),
            idempotent: true,
            projection: None,
            pagination: PackPagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![PackParam {
                name: "X-Request-Id".to_owned(),
                location: PackParamLocation::Header,
                ty: "string".to_owned(),
                required: false,
                default: None,
                fixed: None,
                enum_values: None,
                description: None,
                position: 0,
            }],
            tags: BTreeSet::new(),
            description: None,
        },
    );
    assert!(crate::pack::validate(&pack).is_ok());
}
