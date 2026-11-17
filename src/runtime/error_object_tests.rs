//! `error_object.rs`'s unit tests.

use std::collections::BTreeMap;

use crate::http::CallResponse;
use crate::runtime::dispatch::DispatchOutcome;

use super::leaves::{every_leaf, expected_kind};
use super::*;

#[test]
fn every_dispatch_error_leaf_serializes_to_its_own_kind() {
    let leaves = every_leaf();
    assert!(leaves.len() > 50, "the walk visited every chain");
    for err in leaves {
        assert!(
            serde_json::to_value(&err).is_ok(),
            "unserializable: {err:?}"
        );
        let obj = of_dispatch_error(&err);
        assert_eq!(obj["kind"], json!(expected_kind(&err)), "{err:?}");
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
