//! Unit tests for `bind`. Split out from `bind.rs` to keep that file under the 400-line cap; the
//! proptest lives in the sibling `bind_proptest.rs`.

use super::*;
use crate::model::{Access, Pagination, ParamType, Slug};
use serde_json::json;
use std::str::FromStr;

fn test_service() -> Service {
    Service {
        owner_id: uuid::Uuid::nil(),
        slug: Slug::from_str("demo").expect("valid slug"),
        base_url: url::Url::parse("https://api.example.com").expect("valid url"),
        origin_allowlist: Default::default(),
        default_headers: Default::default(),
        timeout_ms: 5000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1_000_000,
    }
}

fn plain_param(name: &str, position: i32) -> Param {
    Param {
        name: name.to_owned(),
        location: ParamLocation::Path,
        ty: ParamType::String,
        required: true,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position,
    }
}

fn test_call(path_template: &str, params: Vec<Param>) -> ApiCall {
    ApiCall {
        owner_id: uuid::Uuid::nil(),
        slug: Slug::from_str("get-item").expect("valid slug"),
        service_slug: Slug::from_str("demo").expect("valid slug"),
        method: ::http::Method::GET,
        path_template: path_template.to_owned(),
        query_fixed: Default::default(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params,
        description: None,
    }
}

#[test]
fn binds_a_simple_path_param() {
    let service = test_service();
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let mut args = BTreeMap::new();
    args.insert("id".to_owned(), json!("42"));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.url.as_str(), "https://api.example.com/items/42");
    assert_eq!(bound.method, ::http::Method::GET);
    assert!(bound.body.is_none());
}

#[test]
fn query_params_are_sorted_by_key() {
    let service = test_service();
    let mut zebra = plain_param("zebra", 0);
    zebra.location = ParamLocation::Query;
    let mut apple = plain_param("apple", 1);
    apple.location = ParamLocation::Query;
    let call = test_call("/search", vec![zebra, apple]);
    let mut args = BTreeMap::new();
    args.insert("zebra".to_owned(), json!("z"));
    args.insert("apple".to_owned(), json!("a"));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.url.query(), Some("apple=a&zebra=z"));
}

#[test]
fn array_query_param_emits_repeated_keys_in_order() {
    let service = test_service();
    let mut tags = plain_param("tags", 0);
    tags.location = ParamLocation::Query;
    tags.ty = ParamType::StringArray;
    let call = test_call("/search", vec![tags]);
    let mut args = BTreeMap::new();
    args.insert("tags".to_owned(), json!(["b", "a"]));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.url.query(), Some("tags=b&tags=a"));
}

#[test]
fn header_param_outside_allowlist_is_rejected() {
    let service = test_service();
    let mut header = plain_param("Authorization", 0);
    header.location = ParamLocation::Header;
    let call = test_call("/items", vec![header]);
    let mut args = BTreeMap::new();
    args.insert("Authorization".to_owned(), json!("Bearer x"));

    let err = bind(&call, &service, &args).unwrap_err();
    assert!(matches!(err, BindError::HeaderNotAllowed { .. }));
}

#[test]
fn header_value_with_crlf_is_rejected() {
    let service = test_service();
    let mut header = plain_param("X-Request-Id", 0);
    header.location = ParamLocation::Header;
    let call = test_call("/items", vec![header]);
    let mut args = BTreeMap::new();
    args.insert("X-Request-Id".to_owned(), json!("abc\r\nSet-Cookie: x"));

    let err = bind(&call, &service, &args).unwrap_err();
    assert!(matches!(err, BindError::HeaderInvalidValue { .. }));
}

#[test]
fn body_param_is_spliced_at_its_pointer() {
    let service = test_service();
    let mut email = plain_param("email", 0);
    email.location = ParamLocation::Body(PointerBuf::from_tokens(["user", "email"]));
    let mut call = test_call("/users", vec![email]);
    call.method = ::http::Method::POST;
    call.body_template = Some(json!({"user": {"role": "member"}}));
    let mut args = BTreeMap::new();
    args.insert("email".to_owned(), json!("a@example.com"));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(
        bound.body,
        Some(json!({"user": {"role": "member", "email": "a@example.com"}}))
    );
}

#[test]
fn body_param_creates_an_object_when_no_template_is_set() {
    let service = test_service();
    let mut name = plain_param("name", 0);
    name.location = ParamLocation::Body(PointerBuf::from_tokens(["name"]));
    let mut call = test_call("/users", vec![name]);
    call.method = ::http::Method::POST;
    let mut args = BTreeMap::new();
    args.insert("name".to_owned(), json!("Ada"));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.body, Some(json!({"name": "Ada"})));
}

#[test]
fn fixed_query_and_param_query_coexist() {
    let service = test_service();
    let mut call = test_call("/items", vec![]);
    call.query_fixed
        .insert("format".to_owned(), "json".to_owned());
    let args = BTreeMap::new();

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.url.query(), Some("format=json"));
}

#[test]
fn dot_dot_value_cannot_escape_the_declared_segment() {
    let service = test_service();
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let mut args = BTreeMap::new();
    args.insert("id".to_owned(), json!("../secrets"));

    let bound = bind(&call, &service, &args).expect("binds");
    // The value is one opaque, percent-encoded segment — not a '..' navigation.
    assert_eq!(bound.url.path(), "/items/..%2Fsecrets");
    assert_eq!(bound.url.path_segments().expect("has segments").count(), 2);
}

#[test]
fn root_template_with_no_params_binds_to_slash() {
    let service = test_service();
    let call = test_call("/", vec![]);
    let bound = bind(&call, &service, &BTreeMap::new()).expect("binds");
    assert_eq!(bound.url.as_str(), "https://api.example.com/");
}

#[test]
fn array_value_in_a_path_slot_is_rejected() {
    let service = test_service();
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let mut args = BTreeMap::new();
    args.insert("id".to_owned(), json!(["a", "b"]));

    let err = bind(&call, &service, &args).unwrap_err();
    assert!(matches!(err, BindError::UnsupportedValueShape { .. }));
}

#[test]
fn missing_required_path_value_surfaces_as_template_error() {
    let service = test_service();
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let err = bind(&call, &service, &BTreeMap::new()).unwrap_err();
    assert!(matches!(err, BindError::Template(_)));
}

fn service_at(base_url: &str) -> Service {
    Service {
        base_url: url::Url::parse(base_url).expect("valid url"),
        ..test_service()
    }
}

#[test]
fn base_path_is_prepended_with_exactly_one_slash_at_the_join() {
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let mut args = BTreeMap::new();
    args.insert("id".to_owned(), json!("42"));

    for base in [
        "https://api.example.com/api/v1",
        "https://api.example.com/api/v1/",
    ] {
        let bound = bind(&call, &service_at(base), &args).expect("binds");
        assert_eq!(
            bound.url.as_str(),
            "https://api.example.com/api/v1/items/42",
            "base {base}"
        );
    }
}

#[test]
fn a_bare_or_slash_base_path_behaves_exactly_as_an_origin() {
    let call = test_call("/items", vec![]);
    for base in ["https://api.example.com", "https://api.example.com/"] {
        let bound = bind(&call, &service_at(base), &BTreeMap::new()).expect("binds");
        assert_eq!(bound.url.as_str(), "https://api.example.com/items");
    }
}

#[test]
fn root_template_under_a_base_path_binds_to_the_base_directory() {
    let call = test_call("/", vec![]);
    let service = service_at("https://api.example.com/api/v1");
    let bound = bind(&call, &service, &BTreeMap::new()).expect("binds");
    assert_eq!(bound.url.as_str(), "https://api.example.com/api/v1/");
}

#[test]
fn port_and_percent_encoded_base_path_survive_the_join() {
    let call = test_call("/items", vec![]);
    let service = service_at("http://127.0.0.1:8081/my%20api/v1/");
    let bound = bind(&call, &service, &BTreeMap::new()).expect("binds");
    assert_eq!(
        bound.url.as_str(),
        "http://127.0.0.1:8081/my%20api/v1/items"
    );
}

#[test]
fn an_encoded_dot_dot_literal_cannot_climb_out_of_the_base_path() {
    // `UrlTemplate::parse` accepts `%2e%2e` as a well-formed percent-encoded literal, but
    // `Url::parse` treats it as `..` — the containment check is what has to catch it.
    for template in [
        "/%2e%2e/admin",
        "/%2E%2e/admin",
        "/items/%2e%2e/%2e%2e/admin",
    ] {
        let call = test_call(template, vec![]);
        let service = service_at("https://api.example.com/api/v1");
        let err = bind(&call, &service, &BTreeMap::new()).unwrap_err();
        assert_eq!(err, BindError::ContainmentViolation, "template {template}");
    }
}

#[test]
fn a_dot_dot_value_stays_an_opaque_segment_under_the_base_path() {
    let call = test_call("/items/{id}", vec![plain_param("id", 0)]);
    let service = service_at("https://api.example.com/api/v1");
    let mut args = BTreeMap::new();
    args.insert("id".to_owned(), json!("../../admin"));

    let bound = bind(&call, &service, &args).expect("binds");
    assert_eq!(bound.url.path(), "/api/v1/items/..%2F..%2Fadmin");
}

#[test]
fn a_base_url_with_a_query_or_fragment_is_refused_rather_than_dropped() {
    let call = test_call("/items", vec![]);
    for base in [
        "https://api.example.com/api?key=1",
        "https://api.example.com/api#frag",
    ] {
        let err = bind(&call, &service_at(base), &BTreeMap::new()).unwrap_err();
        assert_eq!(err, BindError::BaseUrlHasQueryOrFragment, "base {base}");
    }
}
