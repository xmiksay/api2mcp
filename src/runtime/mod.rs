//! `Executor::run_tool` — the single entry point shared by the MCP data plane, the `invoke`
//! dispatcher and the CLI (all three are later chunks; this one only has to make sure the entry
//! point exists and behaves correctly in isolation).
//!
//! The module is laid out exactly along the seams the plan calls out:
//! - [`budget`] — I6's dynamic half: atomic counters + deadline, built from the plan's already-
//!   folded [`crate::model::Budgets`] before any engine or client exists.
//! - [`dispatch`] — I1's runtime enforcement: the only path in the crate that can reach
//!   `http::send`.
//! - [`fanout`] — I7's concurrent half: runs a batch without ever letting completion order leak
//!   into the result.
//! - [`partial`] — the partial-failure envelope built on top of the above three.
//! - [`snapshot`]/[`recorder`] — what gets persisted, and how it's redacted first.
//!
//! **C9 (Rhai scripting) is deliberately not implemented here.** `dispatch::dispatch` (with
//! `caller_script: Some(script)`) and `partial::run_batch` are the two integration points a
//! script's `api()`/`api_many()` bindings call into — both are `pub`, take no Rhai type, and are
//! exercised directly by this crate's own tests. What C9 still owns: driving a script's *own*
//! sequence of `api()`/`api_many()` calls against one shared [`budget::BudgetMeter`]/
//! [`fanout::ConcurrencyLimits`] pair for the run's whole lifetime, and remapping each batch's
//! locally-0-based [`partial::BatchEntry::index`] into a run-wide, globally unique `run_calls.seq`
//! before more than one batch's worth of entries reach [`recorder::record`] — a concern that
//! cannot arise yet, since every path this chunk exercises is exactly one batch.

pub mod budget;
pub mod dispatch;
pub mod fanout;
pub mod partial;
pub mod recorder;
pub mod snapshot;

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::http::{SsrfPolicy, UpstreamPool};
use crate::resolve::EndpointPlan;
use crate::resolve::plan::ToolTarget;
use crate::store::{RunCallerKind, RunStatus, StoreError, Stores};

use budget::{BudgetAxis, BudgetMeter};
use dispatch::{AuthProviders, DispatchContext};
use fanout::ConcurrencyLimits;
use partial::{BatchStatus, run_batch};
use recorder::RunRecord;

/// Errors from [`Executor::run_tool`] itself — resolving the tool name, loading auth, and
/// persisting the run. Per-item upstream failures never surface here: those are folded into a
/// successful [`RunResult`] with a non-`Ok` [`RunStatus`], per "one bad call never hard-fails a
/// run".
#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("tool {name:?} is not defined on this endpoint")]
    ToolNotFound { name: String },
    #[error("tool {name:?} names a script that is not on this endpoint's plan")]
    ScriptNotOnPlan { name: String },
    #[error("loading auth providers: {0}")]
    Auth(StoreError),
    #[error("recording the run: {0}")]
    Record(StoreError),
}

/// What a completed (or partially-completed) run hands back to its caller.
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub run_id: Uuid,
    pub status: RunStatusView,
    pub value: Option<Value>,
    /// Populated whenever the single item this run dispatched didn't succeed — the direct-
    /// invocation case is always exactly one item, so "the" error is unambiguous here. A future
    /// script-batch run instead has its own `errors[]` array (already persisted by `recorder`) and
    /// wouldn't use this field the same way.
    pub error: Option<String>,
}

/// A `Serialize`-friendly mirror of `store::run::RunStatus` (which deliberately isn't
/// `Serialize` — it's a DB-mapping type, not a model-visible one) for [`RunResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatusView {
    Ok,
    Partial,
    Error,
    Denied,
    BudgetExceeded,
    Timeout,
}

fn to_status_view(status: RunStatus) -> RunStatusView {
    match status {
        RunStatus::Ok => RunStatusView::Ok,
        RunStatus::Partial => RunStatusView::Partial,
        RunStatus::Error => RunStatusView::Error,
        RunStatus::Denied => RunStatusView::Denied,
        RunStatus::BudgetExceeded => RunStatusView::BudgetExceeded,
        RunStatus::Timeout => RunStatusView::Timeout,
    }
}

/// Maps a batch's outcome to a `RunStatus`. The plan pins down budget *mechanics* precisely but
/// not this taxonomy; see `runtime::partial`'s module docs for the judgment call this encodes:
/// a mid-batch budget trip with at least one success is `Partial` (the plan's own wording), a
/// budget trip that wiped out an entire batch is `BudgetExceeded`/`Timeout`, and a batch that
/// failed for ordinary (non-budget) reasons with zero successes is `Error`.
fn to_run_status(status: BatchStatus) -> RunStatus {
    match status {
        BatchStatus::Ok => RunStatus::Ok,
        BatchStatus::Partial => RunStatus::Partial,
        BatchStatus::AllFailedOnBudget(BudgetAxis::WallClock) => RunStatus::Timeout,
        BatchStatus::AllFailedOnBudget(_) => RunStatus::BudgetExceeded,
        BatchStatus::AllFailed => RunStatus::Error,
    }
}

/// The single entry point shared by the MCP data plane, the `invoke` dispatcher and the CLI
/// (later chunks all call through this same type rather than reassembling budget/dispatch/fanout
/// themselves).
pub struct Executor {
    stores: Stores,
    pool: Arc<UpstreamPool>,
    policy: SsrfPolicy,
    max_redirects: u8,
}

impl Executor {
    pub fn new(stores: Stores, pool: Arc<UpstreamPool>, policy: SsrfPolicy) -> Self {
        Self {
            stores,
            pool,
            policy,
            max_redirects: 5,
        }
    }

    /// Runs `tool_name` against `plan` with `args`, recording exactly one `runs` row (plus one
    /// `run_calls` row per attempted upstream request) before returning. Never panics: every
    /// failure this function's own logic can produce (bad name, auth-loading failure, a DB write
    /// failure) is a typed [`ExecutorError`]; every failure the *tool call itself* can produce
    /// (bad arguments, upstream failure, a budget trip) is folded into a successful [`RunResult`]
    /// with a non-`Ok` status instead.
    pub async fn run_tool(
        &self,
        plan: &EndpointPlan,
        tool_name: &str,
        args: Value,
        caller_kind: RunCallerKind,
        caller_id: String,
    ) -> Result<RunResult, ExecutorError> {
        let started = Instant::now();

        let tool = plan
            .tool(tool_name)
            .ok_or_else(|| ExecutorError::ToolNotFound {
                name: tool_name.to_owned(),
            })?;
        let auth = AuthProviders::load(&self.stores.auth_provider(), plan)
            .await
            .map_err(ExecutorError::Auth)?;
        let ctx = DispatchContext {
            pool: &self.pool,
            policy: &self.policy,
            auth: &auth,
            max_redirects: self.max_redirects,
        };
        let limits = ConcurrencyLimits::build(plan, tool.budgets.max_concurrency);
        let meter = BudgetMeter::new(tool.budgets);

        // A script and an api_call differ only in how the value is produced; budgets, auth,
        // concurrency and the audit record are identical, which is why both paths converge on
        // the same recorder call below.
        let (status, value, error, entries) = match &tool.target {
            ToolTarget::Script(script_slug) => {
                let script = plan.scripts.get(script_slug).ok_or_else(|| {
                    ExecutorError::ScriptNotOnPlan {
                        name: tool_name.to_owned(),
                    }
                })?;
                match crate::script::run_script(
                    plan,
                    &ctx,
                    &limits,
                    &meter,
                    script_slug,
                    script,
                    args.clone(),
                )
                .await
                {
                    Ok(run) => (RunStatus::Ok, Some(run.value), None, run.calls),
                    Err(e) => (RunStatus::Error, None, Some(e.to_string()), Vec::new()),
                }
            }
            ToolTarget::ApiCall(_) => {
                let batch = run_batch(
                    plan,
                    &ctx,
                    &limits,
                    &meter,
                    None,
                    vec![(tool_name.to_owned(), args.clone())],
                )
                .await;
                let status = to_run_status(batch.status);
                let value = batch
                    .entries
                    .first()
                    .and_then(|e| e.outcome.dispatch_outcome())
                    .map(|o| o.value.clone());
                let error = if status == RunStatus::Ok {
                    None
                } else {
                    batch.entries.first().map(entry_error_string)
                };
                (status, value, error, batch.entries)
            }
        };

        let request_id = Uuid::new_v4().to_string();
        let run_id = recorder::record(
            &self.stores,
            RunRecord {
                plan,
                tool,
                caller_kind,
                caller_id,
                request_id,
                args,
                output_redacted: value.clone(),
                status,
                entries: &entries,
                meter: &meter,
                elapsed: Some(started.elapsed()),
            },
        )
        .await
        .map_err(ExecutorError::Record)?;

        Ok(RunResult {
            run_id,
            status: to_status_view(status),
            value,
            error,
        })
    }
}

fn entry_error_string(entry: &partial::BatchEntry) -> String {
    match &entry.outcome {
        partial::ItemOutcome::Ok(_) => String::new(),
        partial::ItemOutcome::Failed(e) => e.to_string(),
        partial::ItemOutcome::NotAttempted(axis) => format!("budget exceeded: {axis:?}"),
        partial::ItemOutcome::BudgetCut(axis, _) => format!("budget exceeded: {axis:?}"),
    }
}

/// A generous stand-in for "no run-level wall-clock opinion" — shared by `partial::run_batch`'s
/// own fallback so a run with no `Budgets::wall_clock` still gets *some* finite per-call deadline
/// to hand `http::send` (whose `SendParams::deadline` field is mandatory). The api_call's/
/// service's own `timeout_ms` remains the tighter, real-world bound in that case.
pub(crate) const NO_WALL_CLOCK_BUDGET_FALLBACK: Duration = Duration::from_secs(3600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_view_mirrors_every_run_status_variant() {
        for status in [
            RunStatus::Ok,
            RunStatus::Partial,
            RunStatus::Error,
            RunStatus::Denied,
            RunStatus::BudgetExceeded,
            RunStatus::Timeout,
        ] {
            // Just exercising every arm — a missing match arm would fail to compile, not fail a
            // runtime assertion, but this keeps the mapping under a named test regardless.
            let _ = to_status_view(status);
        }
    }

    #[test]
    fn batch_status_mapping_matches_the_documented_judgment_call() {
        assert_eq!(to_run_status(BatchStatus::Ok), RunStatus::Ok);
        assert_eq!(to_run_status(BatchStatus::Partial), RunStatus::Partial);
        assert_eq!(
            to_run_status(BatchStatus::AllFailedOnBudget(BudgetAxis::WallClock)),
            RunStatus::Timeout
        );
        assert_eq!(
            to_run_status(BatchStatus::AllFailedOnBudget(BudgetAxis::Calls)),
            RunStatus::BudgetExceeded
        );
        assert_eq!(to_run_status(BatchStatus::AllFailed), RunStatus::Error);
    }
}
