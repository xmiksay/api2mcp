//! Writes `runs`/`run_calls` through `store::run`, redacting everything upstream-derived through
//! `http::redact` first — the recorder must never format a header map, URL or body itself.
//!
//! `run_calls.seq` is the **input index** of the batch this run's dispatch came from, never the
//! order results happened to complete or get recorded in — `BatchEntry::index` already *is* that
//! input index (fan_out restores it before this module ever sees the entries), so this module
//! only has to pass it through.

use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::http::{redact_headers, redact_message, redact_url};
use crate::model::Slug;
use crate::resolve::EndpointPlan;
use crate::resolve::plan::PlannedTool;
use crate::store::StoreError;
use crate::store::{NewRun, NewRunCall, RunCallerKind, RunStatus, RunTargetKind, Stores};

use super::budget::{BudgetMeter, snapshot_json};
use super::partial::BatchEntry;
use super::snapshot::build_snapshot;

/// Everything [`record`] needs beyond the batch's own entries — assembled by `Executor::run_tool`
/// (or, once C9 exists, the script-batch equivalent), never derived inside this module.
pub struct RunRecord<'a> {
    pub plan: &'a EndpointPlan,
    pub tool: &'a PlannedTool,
    pub caller_kind: RunCallerKind,
    pub caller_id: String,
    pub request_id: String,
    pub args: Value,
    pub output_redacted: Option<Value>,
    pub status: RunStatus,
    pub entries: &'a [BatchEntry],
    pub meter: &'a BudgetMeter,
    pub elapsed: Option<Duration>,
}

/// Persists one run and its per-request audit rows. Returns the new run's id.
pub async fn record(stores: &Stores, run: RunRecord<'_>) -> Result<Uuid, StoreError> {
    let (target_kind, target_slug) = target_of(run.tool);

    let new_run = NewRun {
        endpoint_slug: run.plan.slug.clone(),
        tool_name: run.tool.name.clone(),
        target_kind,
        target_slug,
        caller_kind: run.caller_kind,
        caller_id: run.caller_id,
        request_id: run.request_id,
        definition_snapshot: build_snapshot(run.plan, run.tool),
        definition_digest: run.plan.digest.clone(),
        input_redacted: run.args,
        output_redacted: run.output_redacted,
        status: run.status,
        errors: errors_json(run.entries),
        calls_made: run.meter.calls_made(),
        bytes_in: run.meter.bytes_in(),
        pages_fetched: run.meter.pages_fetched(),
        budget_snapshot: Some(snapshot_json(run.meter)),
        timings: run
            .elapsed
            .map(|d| json!({"elapsed_ms": d.as_millis() as u64})),
    };

    let calls: Vec<NewRunCall> = run.entries.iter().filter_map(new_run_call).collect();

    stores.run().create(&new_run, &calls).await
}

fn target_of(tool: &PlannedTool) -> (RunTargetKind, Slug) {
    match &tool.target {
        crate::resolve::plan::ToolTarget::ApiCall(slug) => (RunTargetKind::ApiCall, slug.clone()),
        crate::resolve::plan::ToolTarget::Script(slug) => (RunTargetKind::Script, slug.clone()),
    }
}

/// One `run_calls` row per **attempted** upstream request — an entry `dispatch_outcome()` has no
/// value for (never attempted at all, or failed before/without a response) leaves no row, since
/// there is nothing truthful to record about a request that was never actually sent.
fn new_run_call(entry: &BatchEntry) -> Option<NewRunCall> {
    let outcome = entry.outcome.dispatch_outcome()?;
    let first_page = outcome.pages.first();
    let last_page = outcome.pages.last();

    let url_redacted = first_page.map(|p| redact_url(&p.url)).unwrap_or_default();
    let headers_redacted = Some(json!(redact_headers(
        &last_page.map(|p| p.headers.clone()).unwrap_or_default()
    )));
    let body_redacted = last_page.and_then(|p| serde_json::from_slice::<Value>(&p.body).ok());
    let response_bytes = outcome.bytes_in();
    let response_truncated = false; // truncation is a hard error in `http::body` (never silent), so a
    // recorded row never represents a *silently* truncated body.
    let status_code = last_page.map(|p| p.status.as_u16());

    let error = match &entry.outcome {
        super::partial::ItemOutcome::BudgetCut(axis, _) => {
            Some(format!("budget exceeded: {axis:?}"))
        }
        _ => None,
    };

    Some(NewRunCall {
        seq: entry.index as i32,
        api_call_slug: outcome.api_call_slug.clone(),
        service_slug: outcome.service_slug.clone(),
        method: outcome.method.clone(),
        url_redacted,
        headers_redacted,
        body_redacted,
        status_code,
        response_bytes: Some(response_bytes),
        response_truncated,
        error,
        timings: None,
    })
}

/// The run-level `errors[]` envelope: every non-`Ok` entry, keyed by its input index — the plan's
/// own wording for the partial-failure shape.
fn errors_json(entries: &[BatchEntry]) -> Option<Value> {
    let errors: Vec<Value> = entries
        .iter()
        .filter(|e| !e.outcome.is_ok())
        .map(|e| {
            json!({
                "index": e.index,
                "name": e.name,
                "error": error_message(e),
            })
        })
        .collect();
    if errors.is_empty() {
        None
    } else {
        Some(Value::Array(errors))
    }
}

fn error_message(entry: &BatchEntry) -> String {
    match &entry.outcome {
        super::partial::ItemOutcome::Ok(_) => String::new(),
        super::partial::ItemOutcome::Failed(e) => redact_message(&e.to_string()),
        super::partial::ItemOutcome::NotAttempted(axis) => {
            format!("not attempted: budget exceeded ({axis:?})")
        }
        super::partial::ItemOutcome::BudgetCut(axis, _) => {
            format!("budget exceeded ({axis:?})")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn errors_json_is_none_when_every_entry_succeeded() {
        let entries = vec![BatchEntry {
            index: 0,
            name: "a".into(),
            outcome: super::super::partial::ItemOutcome::Ok(sample_outcome()),
        }];
        assert!(errors_json(&entries).is_none());
    }

    #[test]
    fn errors_json_names_the_input_index_of_each_failure() {
        let entries = vec![
            BatchEntry {
                index: 0,
                name: "a".into(),
                outcome: super::super::partial::ItemOutcome::Ok(sample_outcome()),
            },
            BatchEntry {
                index: 1,
                name: "b".into(),
                outcome: super::super::partial::ItemOutcome::NotAttempted(
                    super::super::budget::BudgetAxis::Calls,
                ),
            },
        ];
        let errors = errors_json(&entries).expect("one failure");
        let arr = errors.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["index"], json!(1));
    }

    fn sample_outcome() -> super::super::dispatch::DispatchOutcome {
        super::super::dispatch::DispatchOutcome {
            api_call_slug: "call-a".parse().unwrap(),
            service_slug: "svc".parse().unwrap(),
            method: ::http::Method::GET,
            request_headers: BTreeMap::new(),
            request_body: None,
            pages: vec![],
            value: Value::Null,
        }
    }
}
