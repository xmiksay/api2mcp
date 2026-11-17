use serde_json::json;

use super::*;

fn call_view(raw: Option<Option<Value>>) -> ScriptCallView {
    ScriptCallView {
        seq: 0,
        api_call_slug: "get-user".to_owned(),
        service_slug: "demo".to_owned(),
        status_code: Some(200),
        response_bytes: Some(12),
        raw,
        error: None,
    }
}

#[test]
fn strip_raw_drops_the_body_but_keeps_status_and_size() {
    let mut calls = vec![
        call_view(Some(Some(json!({"id": 1})))),
        call_view(Some(None)),
    ];
    strip_raw(&mut calls);
    for call in &calls {
        let v = serde_json::to_value(call).expect("serializable");
        assert!(v.get("raw").is_none(), "raw must be absent: {v}");
        assert_eq!(v["status_code"], json!(200));
        assert_eq!(v["response_bytes"], json!(12));
    }
}

#[test]
fn included_raw_without_a_recorded_body_serializes_as_null_not_absent() {
    let v = serde_json::to_value(call_view(Some(None))).expect("serializable");
    assert_eq!(v.get("raw"), Some(&Value::Null));
    let v = serde_json::to_value(call_view(Some(Some(json!([1, 2]))))).expect("serializable");
    assert_eq!(v["raw"], json!([1, 2]));
}

#[test]
fn api_call_result_omits_raw_when_not_requested() {
    let result = ApiCallTestResult {
        run_id: Uuid::nil(),
        status: RunStatusView::Ok,
        projected: Some(json!({"id": 1})),
        status_code: Some(200),
        response_bytes: Some(40),
        raw: None,
        error: None,
    };
    let v = serde_json::to_value(&result).expect("serializable");
    assert!(v.get("raw").is_none(), "raw must be absent: {v}");
    assert_eq!(v["status_code"], json!(200));
    assert_eq!(v["response_bytes"], json!(40));
}
