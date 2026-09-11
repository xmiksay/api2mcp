//! The async side of the sync/async bridge. The engine runs under `spawn_blocking`; each
//! `api`/`api_many`/`api_try` call crosses over an `mpsc::UnboundedSender<BindingCall>` and
//! blocks (`oneshot::Receiver::blocking_recv`) for this module's reply — safe precisely because
//! the blocking thread has no entered-runtime guard (it was never `#[tokio::main]`'d; it's a
//! plain OS thread `spawn_blocking` handed the closure to).
//!
//! **I1 is enforced here, not in `script::bindings`'s registration.** [`dispatch::resolve`] is
//! the only place a script-supplied name is checked against the endpoint's declared allowlist,
//! and it's the exact function `runtime::dispatch::dispatch` itself calls to find the api_call —
//! one place to audit, and it's the place that actually resolves the call, not a duplicate check
//! that could drift from it.

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::model::Slug;
use crate::resolve::EndpointPlan;
use crate::runtime::budget::{BudgetAxis, BudgetMeter};
use crate::runtime::dispatch::{self, DispatchContext, DispatchError};
use crate::runtime::fanout::ConcurrencyLimits;
use crate::runtime::partial::{BatchEntry, BatchStatus, ItemOutcome, run_batch};
use std::sync::Mutex;

/// A script-authoring cap on a single `api_many()` call — independent of the run's own dynamic
/// call budget ([`BudgetMeter`]). A batch this large is a property of the script, not of the
/// upstream, so it's refused outright (a throw) rather than folded into the per-item budget-trip
/// machinery `run_batch` already provides.
pub const MAX_BATCH: usize = 500;

/// One `api`/`api_many`/`api_try` invocation crossing the bridge. `batch` has length 1 for
/// `api`/`api_try`; `api_many` sends the whole list at once so it becomes a single
/// [`run_batch`] call — the shared budget/fan-out machinery, not a loop of single calls.
pub struct BindingCall {
    pub name: String,
    pub batch: Vec<Value>,
    pub reply: oneshot::Sender<BridgeReply>,
}

pub enum BridgeReply {
    /// One JSON object per batch item, in input order: `{"ok","index","value"|"error"}` — `value`
    /// present iff `ok`, `error` iff not. Never empty when `Entries` is returned.
    Entries(Vec<Value>),
    /// A script-authoring bug: a name outside I1's allowlist, an unknown api_call, or a batch
    /// over [`MAX_BATCH`]. An ordinary, catchable error on the script side — these are bugs in
    /// the script text, not properties of the upstream a `try`/`catch` idiom should paper over.
    Refused(String),
    /// The run's own wall-clock budget was already exhausted by the time this batch was
    /// serviced. `script::bindings` turns this into `EvalAltResult::ErrorTerminated` rather than
    /// a catchable error — see that module's docs for why a budget-driven refusal must not be
    /// something a `try`/`catch` loop can spin on.
    Terminated(String),
}

/// Services `rx` until every [`BindingCall`] sender has dropped — which happens only once the
/// blocking closure's engine (and every registered binding closure holding a sender clone) has
/// itself been dropped, i.e. once the script has finished running. Never aborted from outside:
/// the wall-clock cap is enforced by `engine::build_engine`'s `on_progress` hook plus this
/// module's own `AllFailedOnBudget(WallClock)` check below, not by dropping this task.
pub async fn service(
    mut rx: mpsc::UnboundedReceiver<BindingCall>,
    plan: &EndpointPlan,
    ctx: &DispatchContext<'_>,
    limits: &ConcurrencyLimits,
    meter: &BudgetMeter,
    caller_script: &Slug,
    audit: &Mutex<Vec<BatchEntry>>,
) {
    while let Some(call) = rx.recv().await {
        let reply = service_one(plan, ctx, limits, meter, caller_script, &call, audit).await;
        // The blocking thread may have already given up waiting (bridge dropped, engine
        // unwinding) — a closed reply channel is not this loop's problem to report.
        let _ = call.reply.send(reply);
    }
}

async fn service_one(
    plan: &EndpointPlan,
    ctx: &DispatchContext<'_>,
    limits: &ConcurrencyLimits,
    meter: &BudgetMeter,
    caller_script: &Slug,
    call: &BindingCall,
    audit: &Mutex<Vec<BatchEntry>>,
) -> BridgeReply {
    if call.batch.len() > MAX_BATCH {
        return BridgeReply::Refused(format!(
            "api_many: batch of {} exceeds the maximum of {MAX_BATCH}",
            call.batch.len()
        ));
    }
    // The pure, no-I/O half of I1: a name absent from `plan.callable_by[caller_script]` (never
    // declared, or declared but not exposed by this endpoint) or naming a script instead of an
    // api_call can never reach `run_batch` at all.
    if let Err(err) = dispatch::resolve(plan, Some(caller_script), &call.name) {
        return BridgeReply::Refused(err.to_string());
    }

    let items: Vec<(String, Value)> = call
        .batch
        .iter()
        .cloned()
        .map(|args| (call.name.clone(), args))
        .collect();
    let outcome = run_batch(plan, ctx, limits, meter, Some(caller_script), items).await;

    // Every upstream call a script makes still has to reach the audit trail, or a script tool
    // would record a run with zero `run_calls` rows and the auditability claim would be false
    // for exactly the tools that do the most. Entries are re-indexed onto one monotonic sequence
    // across the whole script run, because each batch numbers its own items from zero and
    // `run_calls` is keyed `UNIQUE(run_id, seq)`.
    if let Ok(mut sink) = audit.lock() {
        let base = sink.len();
        sink.extend(outcome.entries.iter().enumerate().map(|(i, e)| BatchEntry {
            index: base + i,
            name: e.name.clone(),
            outcome: e.outcome.clone(),
        }));
    }

    // Every item failing on the run's own wall clock (as opposed to one slow call's own
    // per-request timeout, which stays a per-item `Failed` entry) means the deadline was already
    // gone before this batch could even start — see `runtime::partial::run_batch`'s own
    // `remaining_time()` check. That is promoted to termination rather than left as an ordinary
    // catchable per-item failure a `try`/`catch` loop could spin on indefinitely.
    if outcome.status == BatchStatus::AllFailedOnBudget(BudgetAxis::WallClock) {
        return BridgeReply::Terminated("budget exceeded: wall_clock".to_owned());
    }

    BridgeReply::Entries(outcome.entries.iter().map(entry_to_json).collect())
}

/// The script-visible shape of one batch entry: `ok` + `index` always; `value` iff `ok`, `error`
/// iff not.
///
/// `error` is an **object**, not a string: it carries the `kind` tag from
/// [`DispatchError`] so a script can branch on the failure (`if !r.ok && r.error.kind ==
/// "http_status"`) instead of pattern-matching prose. Flattening it to a message would make the
/// per-item error shape useless for anything but logging, which defeats the point of returning
/// per-item errors at all. It is the same serialization a run's persisted `errors[]` uses, so a
/// script's view of a failure and the audit record of it never disagree.
fn entry_to_json(entry: &BatchEntry) -> Value {
    match &entry.outcome {
        ItemOutcome::Ok(outcome) => json!({
            "ok": true,
            "index": entry.index,
            "value": outcome.value,
        }),
        ItemOutcome::Failed(e) => json!({
            "ok": false,
            "index": entry.index,
            "error": error_object(e),
        }),
        ItemOutcome::NotAttempted(axis) | ItemOutcome::BudgetCut(axis, _) => json!({
            "ok": false,
            "index": entry.index,
            "error": budget_error_object(*axis),
        }),
    }
}

/// Serializes a [`DispatchError`] into its tagged object, falling back to a message-only object
/// if serialization somehow fails — a script must always find `kind` and `message` present.
fn error_object(e: &DispatchError) -> Value {
    let mut obj = serde_json::to_value(e).unwrap_or_else(|_| json!({"kind": "internal"}));
    if let Some(map) = obj.as_object_mut() {
        map.insert("message".into(), Value::String(e.to_string()));
    }
    obj
}

fn budget_error_object(axis: BudgetAxis) -> Value {
    json!({
        "kind": "budget_exceeded",
        "axis": axis.as_str(),
        "message": format!("budget exceeded: {}", axis.as_str()),
    })
}
