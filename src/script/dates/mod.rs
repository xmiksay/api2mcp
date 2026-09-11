//! Real timestamp/duration types for scripts, plus the one contrast this module exists to make
//! impossible to miss: [`execution_start`](fn@register)/`now()`.
//!
//! **`execution_start()`** returns a single instant captured once, at the top of the run
//! (`BudgetMeter::new` — see `runtime::budget`'s own doc comment on its `execution_start` field)
//! — constant across every call within that run. A script that derives all its time from this is
//! reproducible: same upstream responses + the same recorded start time = the same output,
//! replayable from the audit log.
//!
//! **`now()`** is the real, moving wall clock — the deliberate escape hatch this module doesn't
//! try to prevent, only make impossible to miss the tradeoff of: using `now()` forfeits
//! reproducibility. `execution_start()` is almost always what a script author actually wants.
//!
//! **Never register anything literally named `timestamp`**, in any arity or as a method on
//! [`Timestamp`] — that name (and `BasicTimePackage`'s wall clock behind it) is the one thing
//! `engine::build_engine` deliberately excludes; see that module's docs and its golden-probe
//! test. The epoch-seconds accessor here is spelled `Timestamp::unix_seconds` for exactly that
//! reason.

mod span;
mod timestamp;

pub use span::Span;
pub use timestamp::Timestamp;

use chrono::{DateTime, Utc};
use rhai::Engine;

/// Registers the `Timestamp`/`Span` types, their methods/operators, and the two script-visible
/// clock functions. `execution_start` is this run's single frozen instant (see module docs) —
/// captured by [`crate::runtime::budget::BudgetMeter::new`] and threaded here by
/// [`super::run_script`] via [`super::engine::build_engine`].
pub fn register(engine: &mut Engine, execution_start: DateTime<Utc>) {
    timestamp::register(engine);
    span::register(engine);

    let start = Timestamp::from(execution_start);
    engine.register_fn("execution_start", move || start);
    engine.register_fn("now", || Timestamp::from(Utc::now()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_start_is_constant_within_one_engine() {
        let mut e = Engine::new_raw();
        register(&mut e, Utc::now());
        let a: Timestamp = e.eval("execution_start()").unwrap();
        let b: Timestamp = e.eval("execution_start()").unwrap();
        assert_eq!(
            a, b,
            "execution_start() must be constant within one engine/run"
        );
    }

    #[test]
    fn now_and_execution_start_are_independent_functions() {
        let mut e = Engine::new_raw();
        register(&mut e, Utc::now());
        // Just proving both names resolve — `now()`'s own moving-clock behaviour is exercised at
        // the integration level (two separate runs), not by racing it against itself here.
        let _: Timestamp = e.eval("now()").unwrap();
        let _: Timestamp = e.eval("execution_start()").unwrap();
    }
}
