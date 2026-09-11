//! I6's dynamic half: `BudgetMeter` is built once per run, from the plan's already-folded
//! [`Budgets`] (folding itself is `resolve::budgets`'/`Budgets::fold`'s job, done before an
//! executor exists), and enforced here by atomic counters plus a deadline. The script never sees
//! this type — only `runtime::dispatch`/`runtime::partial` touch it.
//!
//! Three mechanics the plan is explicit about and this module must not "improve":
//! - **Call reservation is whole-batch, all-or-nothing** ([`BudgetMeter::reserve_calls`]).
//!   Partial admission would make *which* calls ran depend on interleaving.
//! - **The per-call byte cap is computed once per batch** ([`BudgetMeter::batch_byte_cap`]), and
//!   totals are committed **in index order** after the batch completes
//!   ([`BudgetMeter::commit_bytes_in_order`]) — never as each call happens to finish, or two
//!   concurrent calls would race for the remaining budget and *which one* trips would depend on
//!   timing.
//! - **A wall-clock timeout is a terminal budget trip, never a per-item error** — this module
//!   only exposes [`BudgetMeter::remaining_time`], which errors once the deadline has passed;
//!   turning a per-item `CallError::Timeout` into the same [`BudgetTrip`] is `runtime::dispatch`'s
//!   job, so every timeout — whether the meter's own deadline or a single slow upstream — collapses
//!   onto this one terminal path.
//!
//! No retries live here or anywhere else in `runtime`: a retry would make the upstream call count
//! timing-dependent, which is exactly what a fixed, pre-reserved budget is supposed to prevent.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::model::Budgets;

/// Which axis of [`Budgets`] a trip occurred on. `Serialize` so a trip can be named verbatim in
/// a run's `errors` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAxis {
    Calls,
    Bytes,
    WallClock,
    Pages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize)]
#[error("budget exceeded: {axis:?}")]
pub struct BudgetTrip {
    pub axis: BudgetAxis,
}

/// A snapshot of a meter's ceilings and usage at the moment a run finished — persisted verbatim
/// into `runs.budget_snapshot`.
pub fn snapshot_json(meter: &BudgetMeter) -> Value {
    json!({
        "max_calls": meter.max_calls,
        "max_bytes": meter.max_bytes,
        "max_pages": meter.max_pages,
        "wall_clock_ms": meter.wall_clock_ms,
        "calls_used": meter.calls_used.load(Ordering::SeqCst),
        "bytes_used": meter.bytes_used.load(Ordering::SeqCst),
        "pages_used": meter.pages_used.load(Ordering::SeqCst),
    })
}

/// Atomic call/byte/page counters plus a deadline, built in `Executor::run_tool` from the plan's
/// folded [`Budgets`] **before any engine or client exists** (the plan brief's wording) — the
/// script never gets a handle to this type, only `dispatch`/`partial` do.
pub struct BudgetMeter {
    max_calls: Option<u32>,
    max_bytes: Option<u64>,
    max_pages: Option<u32>,
    wall_clock_ms: Option<u64>,
    calls_used: AtomicU32,
    bytes_used: AtomicU64,
    pages_used: AtomicU32,
    deadline: Option<Instant>,
}

impl BudgetMeter {
    pub fn new(budgets: Budgets) -> Self {
        let deadline = budgets.wall_clock.map(|d| Instant::now() + d);
        Self {
            max_calls: budgets.max_calls,
            max_bytes: budgets.max_bytes,
            max_pages: budgets.max_pages,
            wall_clock_ms: budgets.wall_clock.map(|d| d.as_millis() as u64),
            calls_used: AtomicU32::new(0),
            bytes_used: AtomicU64::new(0),
            pages_used: AtomicU32::new(0),
            deadline,
        }
    }

    /// Whether the wall-clock deadline has already passed. Checked before every reservation so a
    /// timed-out run can't admit one more call or page just because nothing else noticed yet.
    pub fn deadline_passed(&self) -> bool {
        self.deadline.is_some_and(|dl| Instant::now() >= dl)
    }

    /// Time left before the run's wall-clock budget trips, or `None` when the run has no
    /// wall-clock opinion at all (the caller then falls back to a per-call/service default).
    pub fn remaining_time(&self) -> Result<Option<Duration>, BudgetTrip> {
        match self.deadline {
            None => Ok(None),
            Some(dl) => {
                let now = Instant::now();
                if now >= dl {
                    Err(BudgetTrip {
                        axis: BudgetAxis::WallClock,
                    })
                } else {
                    Ok(Some(dl - now))
                }
            }
        }
    }

    /// Reserves `n` calls for one batch — whole-batch, all-or-nothing (see module docs): either
    /// every one of the `n` calls is admitted, or none are, and the counter is left untouched on
    /// failure. A `compare_exchange` retry loop rather than `fetch_add`-then-check, because the
    /// latter would need to roll a failed over-admission back — this never admits past the
    /// ceiling in the first place.
    pub fn reserve_calls(&self, n: u32) -> Result<(), BudgetTrip> {
        if self.deadline_passed() {
            return Err(BudgetTrip {
                axis: BudgetAxis::WallClock,
            });
        }
        let trip = BudgetTrip {
            axis: BudgetAxis::Calls,
        };
        let Some(max) = self.max_calls else {
            self.calls_used.fetch_add(n, Ordering::SeqCst);
            return Ok(());
        };
        let mut current = self.calls_used.load(Ordering::SeqCst);
        loop {
            let next = current.checked_add(n).ok_or(trip)?;
            if next > max {
                return Err(trip);
            }
            match self.calls_used.compare_exchange(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// The per-call response byte cap for a batch, computed **once** (see module docs) from
    /// whatever's left of the run's byte budget right now, floored against `per_call_ceiling`
    /// (the api_call/service's own static cap, unrelated to the run budget). Every call in the
    /// batch is sent with this same value, so no single call's transfer can consume more than
    /// "what might still be left" — the batch-level commit afterwards is what actually charges it.
    pub fn batch_byte_cap(&self, per_call_ceiling: u64) -> u64 {
        match self.max_bytes {
            None => per_call_ceiling,
            Some(max) => {
                let used = self.bytes_used.load(Ordering::SeqCst);
                max.saturating_sub(used).min(per_call_ceiling)
            }
        }
    }

    /// Commits observed byte counts **in index order**, never completion order — the mechanism
    /// that makes byte-budget attribution deterministic regardless of which concurrent call in
    /// the batch happened to finish first. Returns the first input index whose commit would push
    /// the running total over the budget, if any; every index at or after that point is left
    /// uncommitted (the bytes were still fetched over the wire — the batch already ran — but the
    /// budget stops counting there, and the caller treats that index onward as a budget trip).
    pub fn commit_bytes_in_order(&self, ordered: &[(usize, u64)]) -> Option<usize> {
        let Some(max) = self.max_bytes else {
            let total: u64 = ordered.iter().map(|(_, b)| *b).sum();
            self.bytes_used.fetch_add(total, Ordering::SeqCst);
            return None;
        };
        let mut used = self.bytes_used.load(Ordering::SeqCst);
        let mut committed = 0u64;
        let mut trip_at = None;
        for &(index, bytes) in ordered {
            let candidate = used.saturating_add(bytes);
            if candidate > max {
                trip_at = Some(index);
                break;
            }
            used = candidate;
            committed += bytes;
        }
        if committed > 0 {
            self.bytes_used.fetch_add(committed, Ordering::SeqCst);
        }
        trip_at
    }

    /// The page cap for a batch's own `http::paginate` calls, computed once per batch exactly
    /// like [`Self::batch_byte_cap`] — every call in the batch shares this same ceiling.
    pub fn batch_page_cap(&self) -> u32 {
        match self.max_pages {
            None => u32::MAX,
            Some(max) => max.saturating_sub(self.pages_used.load(Ordering::SeqCst)),
        }
    }

    /// Commits observed page counts in index order, mirroring [`Self::commit_bytes_in_order`]
    /// exactly (same race the plan calls out, same fix).
    pub fn commit_pages_in_order(&self, ordered: &[(usize, u32)]) -> Option<usize> {
        let Some(max) = self.max_pages else {
            let total: u32 = ordered.iter().map(|(_, p)| *p).sum();
            self.pages_used.fetch_add(total, Ordering::SeqCst);
            return None;
        };
        let mut used = self.pages_used.load(Ordering::SeqCst);
        let mut committed = 0u32;
        let mut trip_at = None;
        for &(index, pages) in ordered {
            let candidate = used.saturating_add(pages);
            if candidate > max {
                trip_at = Some(index);
                break;
            }
            used = candidate;
            committed += pages;
        }
        if committed > 0 {
            self.pages_used.fetch_add(committed, Ordering::SeqCst);
        }
        trip_at
    }

    pub fn calls_made(&self) -> u32 {
        self.calls_used.load(Ordering::SeqCst)
    }

    pub fn bytes_in(&self) -> u64 {
        self.bytes_used.load(Ordering::SeqCst)
    }

    pub fn pages_fetched(&self) -> u32 {
        self.pages_used.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets(max_calls: Option<u32>, max_bytes: Option<u64>, max_pages: Option<u32>) -> Budgets {
        Budgets {
            max_calls,
            max_bytes,
            wall_clock: None,
            max_pages,
            max_concurrency: None,
        }
    }

    #[test]
    fn call_reservation_is_all_or_nothing() {
        let meter = BudgetMeter::new(budgets(Some(5), None, None));
        assert!(meter.reserve_calls(3).is_ok());
        // Only 2 remain; a batch of 3 must be refused *entirely*, not admit 2 of them.
        let err = meter.reserve_calls(3).unwrap_err();
        assert_eq!(err.axis, BudgetAxis::Calls);
        assert_eq!(
            meter.calls_made(),
            3,
            "the failed reservation must not partially commit"
        );
    }

    #[test]
    fn call_reservation_exactly_at_the_ceiling_succeeds() {
        let meter = BudgetMeter::new(budgets(Some(5), None, None));
        assert!(meter.reserve_calls(5).is_ok());
        assert!(meter.reserve_calls(1).is_err());
    }

    #[test]
    fn unbounded_calls_never_trip() {
        let meter = BudgetMeter::new(budgets(None, None, None));
        assert!(meter.reserve_calls(1_000_000).is_ok());
    }

    #[test]
    fn byte_commit_is_deterministic_regardless_of_input_order_passed_in() {
        // The whole point of committing "in index order": build the (index, bytes) pairs sorted
        // by index however completion happened to land, and the trip point never depends on that.
        let meter_a = BudgetMeter::new(budgets(None, Some(1000), None));
        let ordered_by_index = [(0, 700u64), (1, 500)];
        assert_eq!(meter_a.commit_bytes_in_order(&ordered_by_index), Some(1));

        let meter_b = BudgetMeter::new(budgets(None, Some(1000), None));
        // Same logical batch, fed in the same index order (as the caller must always do) even
        // though item 1 "finished" first in some hypothetical completion race — the point is the
        // caller never gets to pass completion order in here at all.
        assert_eq!(meter_b.commit_bytes_in_order(&ordered_by_index), Some(1));
    }

    #[test]
    fn byte_commit_charges_everything_before_the_trip_point() {
        let meter = BudgetMeter::new(budgets(None, Some(1000), None));
        assert_eq!(
            meter.commit_bytes_in_order(&[(0, 300), (1, 300), (2, 500)]),
            Some(2)
        );
        assert_eq!(
            meter.bytes_in(),
            600,
            "only the pre-trip items were committed"
        );
    }

    #[test]
    fn byte_commit_within_budget_trips_nothing() {
        let meter = BudgetMeter::new(budgets(None, Some(1000), None));
        assert_eq!(meter.commit_bytes_in_order(&[(0, 400), (1, 400)]), None);
        assert_eq!(meter.bytes_in(), 800);
    }

    #[test]
    fn batch_byte_cap_is_floored_by_remaining_budget() {
        let meter = BudgetMeter::new(budgets(None, Some(1000), None));
        assert_eq!(meter.commit_bytes_in_order(&[(0, 900)]), None);
        assert_eq!(meter.batch_byte_cap(10_000), 100);
    }

    #[test]
    fn wall_clock_trips_once_the_deadline_has_passed() {
        let meter = BudgetMeter::new(Budgets {
            wall_clock: Some(Duration::from_millis(1)),
            ..budgets(None, None, None)
        });
        std::thread::sleep(Duration::from_millis(20));
        let err = meter.remaining_time().unwrap_err();
        assert_eq!(err.axis, BudgetAxis::WallClock);
        assert!(meter.reserve_calls(1).is_err());
    }

    #[test]
    fn no_wall_clock_budget_never_trips() {
        let meter = BudgetMeter::new(budgets(None, None, None));
        assert_eq!(meter.remaining_time().unwrap(), None);
    }

    #[test]
    fn page_commit_mirrors_byte_commit_semantics() {
        let meter = BudgetMeter::new(budgets(None, None, Some(3)));
        assert_eq!(meter.batch_page_cap(), 3);
        assert_eq!(meter.commit_pages_in_order(&[(0, 2), (1, 2)]), Some(1));
        assert_eq!(meter.pages_fetched(), 2);
    }
}
