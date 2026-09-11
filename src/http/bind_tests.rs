//! Unit tests for `bind`. Split out from `bind.rs` to keep that file under the 400-line cap; the
//! proptest lives in the sibling `bind_proptest.rs`.

use super::*;
use crate::model::{Access, Pagination, ParamType, Slug};
use serde_json::json;
use std::str::FromStr;

fn test_service() -> Service {
    Service {
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
        slug: Slug::from_str("get-item").expect("valid slug"),
        service_slug: Slug::from_str("demo").expect("valid slug"),
        auth_provider_slug: None,
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
