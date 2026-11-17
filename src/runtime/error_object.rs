//! The one tagged `{kind, message, status, ...}` shape every failed call is described by —
//! a script's thrown `api()` exception, an `api_try`/`api_many` element's `error`, a run's
//! persisted `errors[]` and a `script.test` call's `error` all come from here, so none of those
//! surfaces can disagree about the same failure.
//!
//! `status` is always present: the upstream's HTTP status when a response came back, `null`
//! (Rhai `()`) when the call was refused or failed before one did. Scripts run with
//! `set_fail_on_invalid_map_property`, so an optional key would make `e.status == 404` itself
//! throw for every non-HTTP failure.

use serde_json::{Value, json};

use crate::http::redact_message;

use super::budget::BudgetAxis;
use super::dispatch::DispatchError;
use super::partial::ItemOutcome;

/// The tagged error for `outcome` — `None` for `Ok`.
pub fn of_outcome(outcome: &ItemOutcome) -> Option<Value> {
    match outcome {
        ItemOutcome::Ok(_) => None,
        ItemOutcome::Failed(e) => Some(of_dispatch_error(e)),
        ItemOutcome::NotAttempted(axis) => Some(budget(*axis, false, None)),
        ItemOutcome::BudgetCut(axis, o) => Some(budget(
            *axis,
            true,
            o.pages.last().map(|p| p.status.as_u16()),
        )),
    }
}

/// Serializes a [`DispatchError`] — `kind` and any variant-specific fields come from its own
/// `#[serde(tag = "kind")]` derive. `message` is `redact_message`d because this object is
/// persisted in `runs.errors` and returned to scripts (I4); serialization falling back to
/// `internal` still leaves `kind`, `message` and `status` present.
pub fn of_dispatch_error(e: &DispatchError) -> Value {
    let mut obj = serde_json::to_value(e).unwrap_or_else(|_| json!({"kind": "internal"}));
    if let Some(map) = obj.as_object_mut() {
        map.insert(
            "message".into(),
            Value::String(redact_message(&e.to_string())),
        );
        // `HttpStatus` already serialized its own `status`; a `response_not_json`/`projection`
        // failure still had a real response, whose status is on the attempt.
        if !map.contains_key("status") {
            let status = e
                .attempt()
                .and_then(|a| a.pages.last())
                .map(|p| p.status.as_u16());
            map.insert("status".into(), json!(status));
        }
    }
    obj
}

/// A refusal raised by the script bridge itself rather than by dispatch (e.g. an `api_many`
/// batch over the static cap) — same shape, no upstream involved.
pub fn refusal(kind: &str, message: &str) -> Value {
    json!({"kind": kind, "message": message, "status": null})
}

/// `attempted` is `true` for [`ItemOutcome::BudgetCut`] (the request went out before the
/// post-hoc commit excluded it) and `false` for [`ItemOutcome::NotAttempted`] (refused before
/// any request in the batch was made).
fn budget(axis: BudgetAxis, attempted: bool, status: Option<u16>) -> Value {
    let message = if attempted {
        format!("budget exceeded: {}", axis.as_str())
    } else {
        format!("not attempted: budget exceeded ({})", axis.as_str())
    };
    json!({
        "kind": "budget_exceeded",
        "axis": axis.as_str(),
        "attempted": attempted,
        "message": message,
        "status": status,
    })
}

#[cfg(test)]
mod tests {
    use crate::schema::ValidationError;

    use super::*;

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
    fn every_dispatch_error_variant_serializes_to_its_own_kind() {
        // An internally tagged newtype over a bare string cannot serialize; that used to turn a
        // client-build failure into `kind: "internal"`.
        let cases = [
            (
                DispatchError::Client {
                    message: "x".into(),
                },
                "client",
            ),
            (DispatchError::Args(ValidationError::NotAnObject), "args"),
            (
                DispatchError::Args(ValidationError::UnknownParam { param: "p".into() }),
                "args",
            ),
        ];
        for (err, kind) in cases {
            assert_eq!(of_dispatch_error(&err)["kind"], json!(kind), "{err:?}");
        }
    }

    #[test]
    fn distinguishes_never_attempted_from_budget_cut() {
        let not_attempted = of_outcome(&ItemOutcome::NotAttempted(BudgetAxis::Pages)).unwrap();
        assert_eq!(not_attempted["kind"], json!("budget_exceeded"));
        assert_eq!(not_attempted["axis"], json!("pages"));
        assert_eq!(not_attempted["attempted"], json!(false));
        assert_eq!(not_attempted.get("status"), Some(&Value::Null));
    }

    #[test]
    fn a_refusal_has_the_same_three_keys() {
        let obj = refusal("batch_too_large", "too many");
        assert_eq!(obj["kind"], json!("batch_too_large"));
        assert_eq!(obj["message"], json!("too many"));
        assert_eq!(obj.get("status"), Some(&Value::Null));
    }
}
