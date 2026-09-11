//! `call.rs`'s unit tests, split out purely to keep that file under the workspace's 400-line
//! cap — see `runtime/dispatch.rs`'s identical `#[path = "dispatch_tests.rs"]` split for
//! precedent.

use std::collections::BTreeMap;

use super::*;

fn slug(s: &str) -> Slug {
    s.parse().unwrap()
}

fn sample_outcome(
    status: ::http::StatusCode,
    body: &[u8],
) -> crate::runtime::dispatch::DispatchOutcome {
    crate::runtime::dispatch::DispatchOutcome {
        api_call_slug: slug("call-a"),
        service_slug: slug("svc"),
        method: ::http::Method::GET,
        request_headers: BTreeMap::new(),
        request_body: None,
        pages: vec![CallResponse {
            status,
            headers: ::http::HeaderMap::new(),
            url: "https://example.com/x".parse().unwrap(),
            body: body.to_vec(),
        }],
        value: Value::String("projected".into()),
    }
}

#[test]
fn batch_status_mapping_matches_the_documented_judgment_call() {
    assert_eq!(batch_status_to_run_status(BatchStatus::Ok), RunStatus::Ok);
    assert_eq!(
        batch_status_to_run_status(BatchStatus::Partial),
        RunStatus::Partial
    );
    assert_eq!(
        batch_status_to_run_status(BatchStatus::AllFailedOnBudget(
            crate::runtime::budget::BudgetAxis::WallClock
        )),
        RunStatus::Timeout
    );
    assert_eq!(
        batch_status_to_run_status(BatchStatus::AllFailedOnBudget(
            crate::runtime::budget::BudgetAxis::Calls
        )),
        RunStatus::BudgetExceeded
    );
    assert_eq!(
        batch_status_to_run_status(BatchStatus::AllFailed),
        RunStatus::Error
    );
}

#[test]
fn raw_from_pages_collapses_a_single_page_to_its_own_value() {
    let pages = vec![CallResponse {
        status: ::http::StatusCode::OK,
        headers: ::http::HeaderMap::new(),
        url: "https://example.com/x".parse().unwrap(),
        body: br#"{"a":1}"#.to_vec(),
    }];
    assert_eq!(raw_from_pages(&pages), serde_json::json!({"a": 1}));
}

#[test]
fn raw_from_pages_collapses_multiple_pages_to_an_array() {
    let page = |n: u8| CallResponse {
        status: ::http::StatusCode::OK,
        headers: ::http::HeaderMap::new(),
        url: "https://example.com/x".parse().unwrap(),
        body: format!(r#"{{"n":{n}}}"#).into_bytes(),
    };
    let got = raw_from_pages(&[page(1), page(2)]);
    assert_eq!(got, serde_json::json!([{"n": 1}, {"n": 2}]));
}

#[test]
fn raw_from_pages_falls_back_to_text_for_a_non_json_body() {
    let pages = vec![CallResponse {
        status: ::http::StatusCode::OK,
        headers: ::http::HeaderMap::new(),
        url: "https://example.com/x".parse().unwrap(),
        body: b"not json".to_vec(),
    }];
    assert_eq!(raw_from_pages(&pages), Value::String("not json".into()));
}

#[test]
fn entry_error_string_names_the_budget_axis() {
    let entry = BatchEntry {
        index: 0,
        name: "a".into(),
        outcome: ItemOutcome::NotAttempted(crate::runtime::budget::BudgetAxis::Calls),
    };
    assert_eq!(entry_error_string(&entry), "budget exceeded: calls");
}

#[test]
fn format_call_entry_reports_ok_with_status_and_bytes() {
    let entry = BatchEntry {
        index: 2,
        name: "get".into(),
        outcome: ItemOutcome::Ok(sample_outcome(::http::StatusCode::OK, b"{}")),
    };
    let line = format_call_entry(&entry);
    assert!(line.starts_with("[2] get (call-a)"));
    assert!(line.contains("[200]"));
    assert!(line.contains("ok, 2 bytes"));
}

#[test]
fn render_reports_a_missing_value_explicitly() {
    assert_eq!(render(None), "(no value)");
}
