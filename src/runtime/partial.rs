//! The partial-failure envelope: one bad call never hard-fails a run.
//!
//! [`run_batch`] is the shared engine behind both a direct tool invocation (a batch of exactly
//! one item, `caller_script = None`) and a script's `api_many()` (a batch of N, `caller_script =
//! Some(script)`, C9's job to call this) — the "fan-out entry point" this chunk's brief asks for.
//! It always returns exactly `items.len()` [`BatchEntry`] values, one per input index, whether
//! that item ended up `Ok`, an ordinary per-item failure, or a casualty of a budget trip.
//!
//! A budget trip is a **run-level stop**: once the whole-batch call reservation fails, or the
//! post-hoc byte/page commit trips at some index, every item from that point is marked as not
//! attempted / cut — never retried, never partially charged. See `runtime::budget` for the
//! mechanics this module is built on top of.

use serde_json::Value;

use crate::model::Slug;
use crate::resolve::EndpointPlan;

use super::budget::{BudgetAxis, BudgetMeter};
use super::dispatch::{CallBudget, DispatchContext, DispatchError, DispatchOutcome, dispatch};
use super::fanout::{BoxedCall, ConcurrencyLimits, fan_out};

/// One item's fate within a batch.
#[derive(Debug, Clone)]
pub enum ItemOutcome {
    Ok(DispatchOutcome),
    /// Ran (or failed to resolve/bind) for a reason unrelated to the run's budget.
    Failed(DispatchError),
    /// Never attempted at all — the whole-batch call reservation failed before any HTTP call in
    /// this batch could be made.
    NotAttempted(BudgetAxis),
    /// The HTTP call actually ran and succeeded, but the post-hoc, in-index-order budget commit
    /// (see `BudgetMeter::commit_bytes_in_order`/`commit_pages_in_order`) excluded it. The
    /// completed [`DispatchOutcome`] is kept (not discarded) so the recorder can still write an
    /// accurate `run_calls` row for a request that genuinely went out over the wire, even though
    /// its result doesn't count toward the tool's returned `results`.
    BudgetCut(BudgetAxis, DispatchOutcome),
}

impl ItemOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, ItemOutcome::Ok(_))
    }

    pub fn budget_axis(&self) -> Option<BudgetAxis> {
        match self {
            ItemOutcome::NotAttempted(axis) | ItemOutcome::BudgetCut(axis, _) => Some(*axis),
            ItemOutcome::Failed(e) => e.budget_trip(),
            ItemOutcome::Ok(_) => None,
        }
    }

    /// The dispatch outcome an actually-sent request produced, if any — `Ok`/`BudgetCut` both
    /// carry one; `Failed`/`NotAttempted` never do.
    pub fn dispatch_outcome(&self) -> Option<&DispatchOutcome> {
        match self {
            ItemOutcome::Ok(o) | ItemOutcome::BudgetCut(_, o) => Some(o),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatchEntry {
    pub index: usize,
    pub name: String,
    pub outcome: ItemOutcome,
}

/// The run-level verdict [`run_batch`] hands back, for `Executor::run_tool` to turn into a
/// `store::run::RunStatus`. See the chunk report for how each variant maps — the plan pins down
/// the budget *mechanics* but not this status taxonomy, so this is a documented judgment call,
/// not something re-derived from ambiguous text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStatus {
    /// Every item succeeded, no budget trip.
    Ok,
    /// At least one item succeeded and at least one did not, for any reason — including a budget
    /// trip mid-batch. Matches the plan's own wording: "a budget trip is a run-level stop ...
    /// run status is partial."
    Partial,
    /// Nothing in the batch succeeded, and every failure traces back to the named budget axis.
    AllFailedOnBudget(BudgetAxis),
    /// Nothing in the batch succeeded, and none of the failures were budget-related.
    AllFailed,
}

pub struct BatchOutcome {
    pub entries: Vec<BatchEntry>,
    pub status: BatchStatus,
}

/// Runs `items` (name, args) as one batch: whole-batch call reservation, a shared byte/page cap
/// and deadline computed once, concurrent fan-out, then an in-index-order budget commit. Always
/// returns `items.len()` entries.
pub async fn run_batch(
    plan: &EndpointPlan,
    ctx: &DispatchContext<'_>,
    limits: &ConcurrencyLimits,
    meter: &BudgetMeter,
    caller_script: Option<&Slug>,
    items: Vec<(String, Value)>,
) -> BatchOutcome {
    let n = items.len();
    if n == 0 {
        return BatchOutcome {
            entries: Vec::new(),
            status: BatchStatus::Ok,
        };
    }

    if let Err(trip) = meter.reserve_calls(n as u32) {
        return not_attempted(items, trip.axis);
    }

    let deadline = match meter.remaining_time() {
        Ok(Some(d)) => d,
        // No run-level wall-clock opinion: fall back to a generous ceiling. The api_call's/
        // service's own `timeout_ms` still bounds each request independently, both via
        // `UpstreamPool`'s client-level connect/read timeouts and `send`'s per-hop `.timeout()`.
        Ok(None) => super::NO_WALL_CLOCK_BUDGET_FALLBACK,
        Err(trip) => return not_attempted(items, trip.axis),
    };
    let call_budget = CallBudget {
        max_response_bytes: meter.batch_byte_cap(u64::MAX),
        max_pages: meter.batch_page_cap().max(1),
        deadline,
    };

    let names: Vec<String> = items.iter().map(|(name, _)| name.clone()).collect();
    let mut fan_items: Vec<(Slug, BoxedCall<'_, Result<DispatchOutcome, DispatchError>>)> =
        Vec::with_capacity(n);
    for (name, args) in items {
        let service_slug = super::dispatch::resolve(plan, caller_script, &name)
            .map(|planned| planned.service.slug.clone())
            .unwrap_or_else(|_| plan.slug.clone());
        let name_owned = name;
        let fut: BoxedCall<'_, Result<DispatchOutcome, DispatchError>> = Box::pin(async move {
            dispatch(plan, ctx, caller_script, &name_owned, &args, call_budget).await
        });
        fan_items.push((service_slug, fut));
    }

    let results = fan_out(limits, fan_items).await;

    // Bytes and pages are committed in **index order**, never completion order — `fan_out`
    // already restored input order, so a plain `enumerate` here is exactly that order.
    let byte_totals: Vec<(usize, u64)> = results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().ok().map(|o| (i, o.bytes_in())))
        .collect();
    let byte_trip_at = meter.commit_bytes_in_order(&byte_totals);

    let page_totals: Vec<(usize, u32)> = results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().ok().map(|o| (i, o.pages_fetched())))
        .collect();
    let page_trip_at = meter.commit_pages_in_order(&page_totals);

    // Two independent axes can each trip at their own index; whichever trips at the *lower*
    // index is the one that actually cuts the batch first, so that's the axis reported.
    let cut: Option<(usize, BudgetAxis)> = match (byte_trip_at, page_trip_at) {
        (Some(b), Some(p)) if p < b => Some((p, BudgetAxis::Pages)),
        (Some(b), _) => Some((b, BudgetAxis::Bytes)),
        (None, Some(p)) => Some((p, BudgetAxis::Pages)),
        (None, None) => None,
    };

    let entries: Vec<BatchEntry> = names
        .into_iter()
        .zip(results)
        .enumerate()
        .map(|(index, (name, result))| {
            let outcome = match result {
                Ok(outcome) => match cut {
                    Some((cut_index, axis)) if index >= cut_index => {
                        ItemOutcome::BudgetCut(axis, outcome)
                    }
                    _ => ItemOutcome::Ok(outcome),
                },
                // A per-item `DispatchError` that's actually a wall-clock/page budget trip (see
                // `DispatchError::budget_trip`, inspected by `ItemOutcome::budget_axis` below)
                // never carries a `DispatchOutcome` — the request that timed out or hit its page
                // cap produced no completed response to record, so it stays `Failed` either way.
                Err(err) => ItemOutcome::Failed(err),
            };
            BatchEntry {
                index,
                name,
                outcome,
            }
        })
        .collect();

    BatchOutcome {
        status: derive_status(&entries),
        entries,
    }
}

fn not_attempted(items: Vec<(String, Value)>, axis: BudgetAxis) -> BatchOutcome {
    let entries = items
        .into_iter()
        .enumerate()
        .map(|(index, (name, _))| BatchEntry {
            index,
            name,
            outcome: ItemOutcome::NotAttempted(axis),
        })
        .collect();
    BatchOutcome {
        entries,
        status: BatchStatus::AllFailedOnBudget(axis),
    }
}

fn derive_status(entries: &[BatchEntry]) -> BatchStatus {
    let succeeded = entries.iter().filter(|e| e.outcome.is_ok()).count();
    if succeeded == entries.len() {
        return BatchStatus::Ok;
    }
    if succeeded > 0 {
        return BatchStatus::Partial;
    }
    // Nothing succeeded: BudgetExceeded/Timeout only when *every* failure traces to the same
    // budget trip — a batch that failed for a mix of ordinary and budget reasons with zero
    // successes is reported as a plain failure, not a budget one, since the budget wasn't the
    // sole cause.
    let axes: Vec<Option<BudgetAxis>> = entries.iter().map(|e| e.outcome.budget_axis()).collect();
    if let Some(first) = axes.first().copied().flatten()
        && axes.iter().all(|a| *a == Some(first))
    {
        return BatchStatus::AllFailedOnBudget(first);
    }
    BatchStatus::AllFailed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_succeeded_is_ok() {
        let entries = vec![ok_entry(0), ok_entry(1)];
        assert_eq!(derive_status(&entries), BatchStatus::Ok);
    }

    #[test]
    fn a_mix_of_ok_and_not_attempted_is_partial_per_the_plans_own_wording() {
        let entries = vec![
            ok_entry(0),
            BatchEntry {
                index: 1,
                name: "b".into(),
                outcome: ItemOutcome::NotAttempted(BudgetAxis::Calls),
            },
        ];
        assert_eq!(derive_status(&entries), BatchStatus::Partial);
    }

    #[test]
    fn everything_cut_by_the_same_budget_axis_with_no_successes_is_all_failed_on_budget() {
        let entries = vec![
            BatchEntry {
                index: 0,
                name: "a".into(),
                outcome: ItemOutcome::NotAttempted(BudgetAxis::Calls),
            },
            BatchEntry {
                index: 1,
                name: "b".into(),
                outcome: ItemOutcome::NotAttempted(BudgetAxis::Calls),
            },
        ];
        assert_eq!(
            derive_status(&entries),
            BatchStatus::AllFailedOnBudget(BudgetAxis::Calls)
        );
    }

    #[test]
    fn ordinary_failures_with_no_successes_are_all_failed_not_a_budget_status() {
        let entries = vec![BatchEntry {
            index: 0,
            name: "a".into(),
            outcome: ItemOutcome::Failed(DispatchError::NotDeclared { name: "a".into() }),
        }];
        assert_eq!(derive_status(&entries), BatchStatus::AllFailed);
    }

    fn ok_entry(index: usize) -> BatchEntry {
        BatchEntry {
            index,
            name: format!("item-{index}"),
            outcome: ItemOutcome::Ok(DispatchOutcome {
                api_call_slug: "call-a".parse().unwrap(),
                service_slug: "svc".parse().unwrap(),
                method: ::http::Method::GET,
                request_headers: std::collections::BTreeMap::new(),
                request_body: None,
                pages: vec![],
                value: Value::Null,
            }),
        }
    }
}
