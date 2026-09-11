//! `Span` — a script-visible duration wrapping `chrono::TimeDelta` (what `chrono::Duration` is
//! now an alias for). Constructed by the free functions below, never a bare integer of
//! milliseconds, so `timestamp + days(1)` reads the way it's meant to.
//!
//! Every constructor and every `+`/`-` overload here goes through `chrono`'s own `try_*`/
//! `checked_*` API rather than the panicking `TimeDelta::days`/`Add`/`Sub` impls — an
//! overflowing argument (a script-authored `i64`, on an I/O-reachable path) becomes a catchable
//! runtime error, never a panic.

use chrono::TimeDelta;
use rhai::{Engine, EvalAltResult, NativeCallContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span(TimeDelta);

impl Span {
    pub fn inner(self) -> TimeDelta {
        self.0
    }
}

impl From<TimeDelta> for Span {
    fn from(td: TimeDelta) -> Self {
        Self(td)
    }
}

fn runtime_err(ctx: &NativeCallContext, msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.to_owned().into(),
        ctx.call_position(),
    ))
}

fn constructor(
    ctx: NativeCallContext,
    unit: &str,
    n: i64,
    f: impl FnOnce(i64) -> Option<TimeDelta>,
) -> Result<Span, Box<EvalAltResult>> {
    f(n).map(Span)
        .ok_or_else(|| runtime_err(&ctx, &format!("{unit}({n}): out of range for a Span")))
}

/// Registers `Span`, its free constructors (`days`/`hours`/`minutes`/`seconds`/`millis`), its
/// accessors (`whole_days`/.../`whole_millis`), and `Span + Span` / `Span - Span`. The
/// `Timestamp <-> Span` operators live in `super::timestamp` instead, since `Timestamp` is
/// already the module that depends on both types.
pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<Span>("Span");

    engine.register_fn("days", |ctx: NativeCallContext, n: i64| {
        constructor(ctx, "days", n, TimeDelta::try_days)
    });
    engine.register_fn("hours", |ctx: NativeCallContext, n: i64| {
        constructor(ctx, "hours", n, TimeDelta::try_hours)
    });
    engine.register_fn("minutes", |ctx: NativeCallContext, n: i64| {
        constructor(ctx, "minutes", n, TimeDelta::try_minutes)
    });
    engine.register_fn("seconds", |ctx: NativeCallContext, n: i64| {
        constructor(ctx, "seconds", n, TimeDelta::try_seconds)
    });
    engine.register_fn("millis", |ctx: NativeCallContext, n: i64| {
        constructor(ctx, "millis", n, TimeDelta::try_milliseconds)
    });

    engine.register_fn("whole_days", |s: &mut Span| s.0.num_days());
    engine.register_fn("whole_hours", |s: &mut Span| s.0.num_hours());
    engine.register_fn("whole_minutes", |s: &mut Span| s.0.num_minutes());
    engine.register_fn("whole_seconds", |s: &mut Span| s.0.num_seconds());
    engine.register_fn("whole_millis", |s: &mut Span| s.0.num_milliseconds());

    engine.register_fn(
        "+",
        |ctx: NativeCallContext, a: Span, b: Span| -> Result<Span, Box<EvalAltResult>> {
            a.0.checked_add(&b.0)
                .map(Span)
                .ok_or_else(|| runtime_err(&ctx, "span addition overflowed"))
        },
    );
    engine.register_fn(
        "-",
        |ctx: NativeCallContext, a: Span, b: Span| -> Result<Span, Box<EvalAltResult>> {
            a.0.checked_sub(&b.0)
                .map(Span)
                .ok_or_else(|| runtime_err(&ctx, "span subtraction overflowed"))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new_raw();
        register(&mut e);
        e
    }

    #[test]
    fn constructors_build_the_expected_duration() {
        let e = engine();
        let s: Span = e.eval("days(2)").unwrap();
        assert_eq!(s.inner().num_hours(), 48);
    }

    #[test]
    fn accessors_read_back_the_same_unit() {
        let e = engine();
        assert_eq!(e.eval::<i64>("hours(3).whole_minutes()").unwrap(), 180);
        assert_eq!(e.eval::<i64>("seconds(90).whole_seconds()").unwrap(), 90);
    }

    #[test]
    fn addition_and_subtraction_compose_spans() {
        let e = engine();
        assert_eq!(
            e.eval::<i64>("(hours(1) + minutes(30)).whole_minutes()")
                .unwrap(),
            90
        );
        assert_eq!(
            e.eval::<i64>("(hours(2) - minutes(15)).whole_minutes()")
                .unwrap(),
            105
        );
    }

    #[test]
    fn an_out_of_range_constructor_is_a_catchable_error_not_a_panic() {
        let e = engine();
        let err = e
            .eval::<Span>(&format!("millis({})", i64::MIN))
            .unwrap_err();
        assert!(matches!(*err, rhai::EvalAltResult::ErrorRuntime(..)));
    }
}
