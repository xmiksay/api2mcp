//! Resource ceilings for a run. `None` means "no opinion" — an axis a definer didn't bother to
//! cap — never "unlimited" in the sense of overriding a narrower opinion elsewhere. That
//! asymmetry is what makes [`Budgets::fold`] a narrowing-only operation (I6): a script's budget
//! can tighten its endpoint's, never loosen it.

use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Budgets {
    pub max_calls: Option<u32>,
    pub max_bytes: Option<u64>,
    pub wall_clock: Option<Duration>,
    pub max_pages: Option<u32>,
    pub max_concurrency: Option<u32>,
}

impl Budgets {
    /// Element-wise `min`, with `None` meaning "no opinion": `Some` always beats `None`, and two
    /// `Some`s keep the smaller. Folding an endpoint's budget with a script's therefore can only
    /// ever narrow the result — the lattice has no way to produce a wider ceiling than either
    /// input.
    pub fn fold(a: Budgets, b: Budgets) -> Budgets {
        Budgets {
            max_calls: fold_opt(a.max_calls, b.max_calls),
            max_bytes: fold_opt(a.max_bytes, b.max_bytes),
            wall_clock: fold_opt(a.wall_clock, b.wall_clock),
            max_pages: fold_opt(a.max_pages, b.max_pages),
            max_concurrency: fold_opt(a.max_concurrency, b.max_concurrency),
        }
    }
}

fn fold_opt<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Budgets {
        Budgets {
            max_calls: Some(10),
            max_bytes: Some(1_000_000),
            wall_clock: Some(Duration::from_secs(30)),
            max_pages: Some(5),
            max_concurrency: Some(4),
        }
    }

    #[test]
    fn identity_with_default_is_the_original() {
        let a = sample();
        assert_eq!(Budgets::fold(a, Budgets::default()), a);
        assert_eq!(Budgets::fold(Budgets::default(), a), a);
    }

    #[test]
    fn commutative() {
        let a = sample();
        let b = Budgets {
            max_calls: Some(3),
            max_bytes: Some(2_000_000),
            wall_clock: Some(Duration::from_secs(5)),
            max_pages: Some(20),
            max_concurrency: Some(1),
        };
        assert_eq!(Budgets::fold(a, b), Budgets::fold(b, a));
    }

    #[test]
    fn some_always_wins_over_none() {
        let narrow = Budgets {
            max_calls: Some(1),
            ..Budgets::default()
        };
        let wide_opinionless = Budgets::default();
        let folded = Budgets::fold(narrow, wide_opinionless);
        assert_eq!(folded.max_calls, Some(1));
    }

    #[test]
    fn two_somes_keep_the_smaller() {
        let a = Budgets {
            max_calls: Some(10),
            ..Budgets::default()
        };
        let b = Budgets {
            max_calls: Some(3),
            ..Budgets::default()
        };
        assert_eq!(Budgets::fold(a, b).max_calls, Some(3));
    }

    #[test]
    fn folding_never_widens_a_narrower_input() {
        let endpoint_budget = Budgets {
            max_calls: Some(50),
            max_bytes: Some(5_000_000),
            wall_clock: Some(Duration::from_secs(60)),
            max_pages: Some(10),
            max_concurrency: Some(8),
        };
        let script_budget = Budgets {
            max_calls: Some(5),
            ..Budgets::default()
        };
        let folded = Budgets::fold(endpoint_budget, script_budget);
        assert!(folded.max_calls.unwrap() <= endpoint_budget.max_calls.unwrap());
        assert_eq!(folded.max_bytes, endpoint_budget.max_bytes);
    }
}
