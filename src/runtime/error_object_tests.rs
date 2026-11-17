//! `error_object.rs`'s unit tests — split out because the every-leaf-variant table is long.

use std::collections::BTreeMap;

use crate::http::{
    AuthError, BindError, BodyError, CallError, CallResponse, CredError, GuardError, PaginateError,
    TemplateError,
};

use crate::project::ProjectionError;
use crate::runtime::dispatch::DispatchOutcome;
use crate::schema::ValidationError;

use super::*;

fn s(v: &str) -> String {
    v.to_owned()
}

/// Every leaf of every tagged enum reachable from [`DispatchError`]. An internally tagged
/// newtype over a bare `String` cannot serialize and silently became `kind: "internal"` —
/// listing each leaf is what catches a new variant of that shape.
fn every_leaf() -> Vec<(DispatchError, &'static str)> {
    let template = [
        TemplateError::NotAbsolute { template: s("t") },
        TemplateError::DoubleSlashStart { template: s("t") },
        TemplateError::ContainsQueryOrFragment { template: s("t") },
        TemplateError::ContainsSchemeSeparator { template: s("t") },
        TemplateError::MixedSegment { segment: s("x") },
        TemplateError::DotSegment { segment: s("..") },
        TemplateError::EmptySegment,
        TemplateError::DuplicatePlaceholder {
            placeholder: s("id"),
        },
        TemplateError::MalformedPercentEncoding { segment: s("%zz") },
        TemplateError::MissingValue {
            placeholder: s("id"),
        },
        TemplateError::EmptyValue {
            placeholder: s("id"),
        },
        TemplateError::DotValue {
            placeholder: s("id"),
        },
        TemplateError::ControlCharacter {
            placeholder: s("id"),
        },
    ];
    let bind = [
        BindError::HeaderNotAllowed { name: s("h") },
        BindError::HeaderInvalidValue { name: s("h") },
        BindError::UnsupportedValueShape {
            name: s("p"),
            shape: "array",
        },
        BindError::LocalParamNotBindable { name: s("p") },
        BindError::BodyAssign {
            pointer: s("/a"),
            message: s("m"),
        },
        BindError::NoHost,
        BindError::UrlParse { message: s("m") },
        BindError::ContainmentViolation,
    ];
    let args = [
        ValidationError::NotAnObject,
        ValidationError::UnknownParam { param: s("p") },
        ValidationError::MissingRequired { param: s("p") },
        ValidationError::WrongType {
            param: s("p"),
            expected: "number",
            got: "string",
        },
        ValidationError::NotInEnum { param: s("p") },
    ];
    let call = [
        CallError::Guard(GuardError::UnsupportedScheme { scheme: s("ftp") }),
        CallError::Guard(GuardError::OriginNotAllowed { origin: s("o") }),
        CallError::Guard(GuardError::NoHost),
        CallError::Guard(GuardError::DeniedLiteralIp {
            host: s("10.0.0.1"),
            range: "private",
        }),
        CallError::Auth(AuthError::Credential(CredError::MissingEnvVar {
            env_key: s("K"),
        })),
        CallError::Auth(AuthError::Credential(CredError::InvalidHeaderValue)),
        CallError::Auth(AuthError::Credential(CredError::MissingStoredCredential)),
        CallError::Auth(AuthError::InvalidHeaderName { name: s("h") }),
        CallError::Body(BodyError::ContentLengthExceedsCap { cap: 1, actual: 2 }),
        CallError::Body(BodyError::StreamExceedsCap { cap: 1 }),
        CallError::Body(BodyError::Transport { message: s("m") }),
        CallError::TooManyRedirects { max: 5 },
        CallError::MissingLocation,
        CallError::InvalidLocation { reason: s("r") },
        CallError::Timeout,
        CallError::InvalidHeader,
        CallError::Transport { message: s("m") },
    ];
    let paginate = [
        PaginateError::PageCapExceeded { max: 3 },
        PaginateError::ResponseNotJson { message: s("m") },
        PaginateError::CursorNotString {
            pointer: s("/next"),
            actual: "number",
        },
    ];
    let projection = [
        ProjectionError::InvalidPath {
            name: s("f"),
            path: s("$"),
            message: s("m"),
        },
        ProjectionError::Missing {
            name: s("f"),
            path: s("$.a"),
        },
        ProjectionError::AmbiguousMatch {
            name: s("f"),
            path: s("$.a"),
            count: 2,
        },
        ProjectionError::CoercionFailed {
            name: s("f"),
            actual: "string",
            expected: "number",
        },
    ];

    let mut all = vec![
        (DispatchError::NotDeclared { name: s("a") }, "not_declared"),
        (
            DispatchError::NotAnApiCall { name: s("a") },
            "not_an_api_call",
        ),
        (DispatchError::Client { message: s("m") }, "client"),
        (
            DispatchError::HttpStatus {
                status: 404,
                reason: s("Not Found"),
                detail: s(""),
                attempt: None,
            },
            "http_status",
        ),
        (
            DispatchError::ResponseNotJson {
                message: s("m"),
                attempt: None,
            },
            "response_not_json",
        ),
    ];
    all.extend(template.map(|e| (DispatchError::Bind(BindError::Template(e)), "bind")));
    all.extend(bind.map(|e| (DispatchError::Bind(e), "bind")));
    all.extend(args.map(|e| (DispatchError::Args(e), "args")));
    all.extend(call.map(|e| (DispatchError::Send(PaginateError::Call(e)), "send")));
    all.extend(paginate.map(|e| (DispatchError::Send(e), "send")));
    all.extend(projection.map(|source| {
        (
            DispatchError::Projection {
                source,
                attempt: None,
            },
            "projection",
        )
    }));
    all
}

#[test]
fn every_dispatch_error_leaf_serializes_to_its_own_kind() {
    for (err, kind) in every_leaf() {
        assert!(
            serde_json::to_value(&err).is_ok(),
            "unserializable: {err:?}"
        );
        let obj = of_dispatch_error(&err);
        assert_eq!(obj["kind"], json!(kind), "{err:?}");
        assert!(obj["message"].is_string(), "{err:?}");
        assert!(obj.get("status").is_some(), "{err:?}");
    }
}

#[test]
fn carries_the_dispatch_error_kind_a_message_and_a_null_status() {
    let obj = of_dispatch_error(&DispatchError::NotDeclared { name: "a".into() });
    assert_eq!(obj["kind"], json!("not_declared"));
    assert!(obj["message"].is_string());
    assert_eq!(obj.get("status"), Some(&Value::Null));
}

#[test]
fn http_status_keeps_its_own_status() {
    let obj = of_dispatch_error(&DispatchError::HttpStatus {
        status: 401,
        reason: "Unauthorized".into(),
        detail: String::new(),
        attempt: None,
    });
    assert_eq!(obj["kind"], json!("http_status"));
    assert_eq!(obj["status"], json!(401));
}

#[test]
fn distinguishes_never_attempted_from_budget_cut() {
    let not_attempted = of_outcome(&ItemOutcome::NotAttempted(BudgetAxis::Pages)).unwrap();
    assert_eq!(not_attempted["kind"], json!("budget_exceeded"));
    assert_eq!(not_attempted["axis"], json!("pages"));
    assert_eq!(not_attempted["attempted"], json!(false));
    assert_eq!(not_attempted.get("status"), Some(&Value::Null));

    let page = |status: ::http::StatusCode| CallResponse {
        status,
        headers: ::http::HeaderMap::new(),
        url: "https://api.example.test/x".parse().unwrap(),
        body: Vec::new(),
    };
    let sent = DispatchOutcome {
        api_call_slug: "call-a".parse().unwrap(),
        service_slug: "svc".parse().unwrap(),
        method: ::http::Method::GET,
        request_headers: BTreeMap::new(),
        request_body: None,
        pages: vec![
            page(::http::StatusCode::OK),
            page(::http::StatusCode::PARTIAL_CONTENT),
        ],
        value: Value::Null,
    };
    let cut = of_outcome(&ItemOutcome::BudgetCut(BudgetAxis::Bytes, sent)).unwrap();
    assert_eq!(cut["kind"], json!("budget_exceeded"));
    assert_eq!(cut["axis"], json!("bytes"));
    assert_eq!(cut["attempted"], json!(true));
    assert_eq!(cut["status"], json!(206), "the last page's status");
}

#[test]
fn a_refusal_has_the_same_three_keys() {
    let obj = refusal("batch_too_large", "too many");
    assert_eq!(obj["kind"], json!("batch_too_large"));
    assert_eq!(obj["message"], json!("too many"));
    assert_eq!(obj.get("status"), Some(&Value::Null));
}
