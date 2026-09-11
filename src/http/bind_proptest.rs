use super::*;
use crate::model::{Access, Origin, Pagination, ParamType, Slug};
use proptest::prelude::*;
use std::str::FromStr;

fn service() -> Service {
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

fn call() -> ApiCall {
    ApiCall {
        slug: Slug::from_str("get-item").expect("valid slug"),
        service_slug: Slug::from_str("demo").expect("valid slug"),
        auth_provider_slug: None,
        method: ::http::Method::GET,
        path_template: "/items/{id}".to_owned(),
        query_fixed: Default::default(),
        body_template: None,
        access: Access::Read,
        idempotent: true,
        projection: None,
        pagination: Pagination::None,
        timeout_ms: None,
        max_response_bytes: None,
        params: vec![Param {
            name: "id".to_owned(),
            location: ParamLocation::Path,
            ty: ParamType::String,
            required: true,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        }],
        description: None,
    }
}

/// Biased toward the characters that matter for path-escaping attacks — '/', '.', '%',
/// NUL, non-ASCII — plus a general-purpose `char` strategy so nothing is off the table.
fn attack_biased_string() -> impl Strategy<Value = String> {
    let biased_char = prop_oneof![
        3 => prop::char::any(),
        2 => Just('/'),
        2 => Just('.'),
        2 => Just('%'),
        1 => Just('\0'),
        1 => Just('é'),
        1 => Just('\n'),
    ];
    prop::collection::vec(biased_char, 0..12).prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    /// I3, stated machine-checkably: whatever string a caller supplies for `id`, `bind`
    /// either errors or produces a URL with exactly the template's segment count and the
    /// base's origin.
    #[test]
    fn bind_never_escapes_template_shape(value in attack_biased_string()) {
        let service = service();
        let call = call();
        let base_origin = Origin::of(&service.base_url).expect("valid origin");
        let mut args = BTreeMap::new();
        args.insert("id".to_owned(), Value::String(value));

        if let Ok(bound) = bind(&call, &service, &args) {
            let segment_count = bound
                .url
                .path_segments()
                .map(|segments| segments.count())
                .unwrap_or(0);
            prop_assert_eq!(segment_count, 2);
            let bound_origin = Origin::of(&bound.url).expect("valid origin");
            prop_assert_eq!(bound_origin, base_origin);
        }
    }
}
