//! `Timestamp` — a script-visible instant wrapping `chrono::DateTime<Utc>`, not an RFC 3339
//! string passed around by convention. Parsing, formatting, component access, arithmetic against
//! [`super::Span`], comparison and range checks all live here so a script author never hand-rolls
//! string-based date arithmetic.
//!
//! **Never register anything named `timestamp`** (function or method) — see this module's parent
//! doc comment. The epoch-seconds accessor is [`Timestamp::unix_seconds`] for exactly that reason.

use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike, Utc};
use rhai::{Engine, EvalAltResult, NativeCallContext};

use super::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    pub fn inner(self) -> DateTime<Utc> {
        self.0
    }

    pub fn to_rfc3339(self) -> String {
        self.0.to_rfc3339()
    }
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }
}

fn runtime_err(ctx: &NativeCallContext, msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.to_owned().into(),
        ctx.call_position(),
    ))
}

/// `chrono::DateTime::format`'s `Display` impl returns `Err(fmt::Error)` when the format string
/// contains an unrecognized specifier, and `ToString::to_string`'s blanket impl `.expect()`s a
/// `Display` never doing that — so an untrusted, script-authored format string handed straight to
/// `.format(fmt).to_string()` is a live panic path. Validating the parsed item stream first (the
/// same `StrftimeItems` chrono's own formatter and parser both use) turns that into a catchable
/// error instead.
fn valid_strftime(fmt: &str) -> bool {
    !StrftimeItems::new(fmt).any(|item| item == Item::Error)
}

/// Tries, in order: a full date+time against `fmt`, an offset-aware date+time against `fmt`
/// (`with_timezone(&Utc)` normalizes it), then a date-only match against `fmt` at midnight UTC.
/// None of chrono's `parse_from_str` paths panic on a bad format string (unlike `.format()` — see
/// [`valid_strftime`]'s docs) — an unmatched format is always a plain `Err`, never a panic.
fn parse_with_format(s: &str, fmt: &str) -> Option<DateTime<Utc>> {
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
        return Some(ndt.and_utc());
    }
    if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
        return d.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
    }
    None
}

fn parse_timestamp(ctx: NativeCallContext, s: &str) -> Result<Timestamp, Box<EvalAltResult>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| Timestamp(dt.with_timezone(&Utc)))
        .map_err(|e| {
            runtime_err(
                &ctx,
                &format!("parse_timestamp: {s:?} is not RFC 3339: {e}"),
            )
        })
}

fn parse_date(ctx: NativeCallContext, s: &str, fmt: &str) -> Result<Timestamp, Box<EvalAltResult>> {
    parse_with_format(s, fmt).map(Timestamp).ok_or_else(|| {
        runtime_err(
            &ctx,
            &format!("parse_date: {s:?} does not match format {fmt:?}"),
        )
    })
}

fn format_method(
    ctx: NativeCallContext,
    ts: &mut Timestamp,
    fmt: &str,
) -> Result<String, Box<EvalAltResult>> {
    if !valid_strftime(fmt) {
        return Err(runtime_err(
            &ctx,
            &format!("format: invalid format string {fmt:?}"),
        ));
    }
    Ok(ts.0.format(fmt).to_string())
}

/// Registers `Timestamp`, the free `parse_timestamp`/`parse_date`/`is_valid_timestamp`/
/// `is_valid_date` functions, every `Timestamp` method (formatting, components, `is_between`),
/// comparison operators, and the `Timestamp <-> Span` arithmetic operators.
pub fn register(engine: &mut Engine) {
    engine.register_type_with_name::<Timestamp>("Timestamp");

    engine.register_fn("parse_timestamp", parse_timestamp);
    engine.register_fn("parse_date", parse_date);
    engine.register_fn("is_valid_timestamp", |s: &str| {
        DateTime::parse_from_rfc3339(s).is_ok()
    });
    engine.register_fn("is_valid_date", |s: &str, fmt: &str| {
        parse_with_format(s, fmt).is_some()
    });

    engine.register_fn("to_rfc3339", |t: &mut Timestamp| t.to_rfc3339());
    engine.register_fn("format", format_method);
    engine.register_fn("year", |t: &mut Timestamp| i64::from(t.0.year()));
    engine.register_fn("month", |t: &mut Timestamp| i64::from(t.0.month()));
    engine.register_fn("day", |t: &mut Timestamp| i64::from(t.0.day()));
    engine.register_fn("hour", |t: &mut Timestamp| i64::from(t.0.hour()));
    engine.register_fn("minute", |t: &mut Timestamp| i64::from(t.0.minute()));
    engine.register_fn("second", |t: &mut Timestamp| i64::from(t.0.second()));
    engine.register_fn("weekday", |t: &mut Timestamp| t.0.weekday().to_string());
    // Never `.timestamp()` — see this module's own docs and `engine`'s golden-probe test for why
    // that exact name is reserved for "absent".
    engine.register_fn("unix_seconds", |t: &mut Timestamp| t.0.timestamp());
    engine.register_fn(
        "is_between",
        |t: &mut Timestamp, start: Timestamp, end: Timestamp| t.0 >= start.0 && t.0 <= end.0,
    );

    engine.register_fn("==", |a: Timestamp, b: Timestamp| a == b);
    engine.register_fn("!=", |a: Timestamp, b: Timestamp| a != b);
    engine.register_fn("<", |a: Timestamp, b: Timestamp| a < b);
    engine.register_fn("<=", |a: Timestamp, b: Timestamp| a <= b);
    engine.register_fn(">", |a: Timestamp, b: Timestamp| a > b);
    engine.register_fn(">=", |a: Timestamp, b: Timestamp| a >= b);

    engine.register_fn(
        "+",
        |ctx: NativeCallContext,
         t: Timestamp,
         span: Span|
         -> Result<Timestamp, Box<EvalAltResult>> {
            t.0.checked_add_signed(span.inner())
                .map(Timestamp)
                .ok_or_else(|| runtime_err(&ctx, "timestamp + span overflowed"))
        },
    );
    engine.register_fn(
        "-",
        |ctx: NativeCallContext,
         t: Timestamp,
         span: Span|
         -> Result<Timestamp, Box<EvalAltResult>> {
            t.0.checked_sub_signed(span.inner())
                .map(Timestamp)
                .ok_or_else(|| runtime_err(&ctx, "timestamp - span overflowed"))
        },
    );
    engine.register_fn("-", |a: Timestamp, b: Timestamp| -> Span {
        Span::from(a.0.signed_duration_since(b.0))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::dates::span;

    fn engine() -> Engine {
        let mut e = Engine::new_raw();
        register(&mut e);
        span::register(&mut e);
        e
    }

    #[test]
    fn rfc3339_round_trips() {
        let e = engine();
        let ts: Timestamp = e
            .eval(r#"parse_timestamp("2024-03-05T12:30:00Z")"#)
            .unwrap();
        assert_eq!(ts.to_rfc3339(), "2024-03-05T12:30:00+00:00");
    }

    #[test]
    fn parse_date_with_a_strftime_format() {
        let e = engine();
        let ts: Timestamp = e.eval(r#"parse_date("05/03/2024", "%d/%m/%Y")"#).unwrap();
        assert_eq!((ts.0.year(), ts.0.month(), ts.0.day()), (2024, 3, 5));
    }

    #[test]
    fn parse_date_failure_is_catchable_and_names_the_input() {
        let e = engine();
        let err = e
            .eval::<Timestamp>(r#"parse_date("nope", "%Y-%m-%d")"#)
            .unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn validation_returns_false_instead_of_throwing() {
        let e = engine();
        assert!(
            !e.eval::<bool>(r#"is_valid_timestamp("not a date")"#)
                .unwrap()
        );
        assert!(
            !e.eval::<bool>(r#"is_valid_date("not a date", "%Y-%m-%d")"#)
                .unwrap()
        );
        assert!(
            e.eval::<bool>(r#"is_valid_date("2024-03-05", "%Y-%m-%d")"#)
                .unwrap()
        );
    }

    #[test]
    fn components_and_weekday_are_readable() {
        let e = engine();
        let src = r#"let t = parse_timestamp("2024-03-05T08:09:10Z"); [t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second(), t.weekday()]"#;
        let arr = e.eval::<rhai::Array>(src).unwrap();
        assert_eq!(arr[0].as_int().unwrap(), 2024);
        assert_eq!(arr[6].clone().into_string().unwrap(), "Tue");
    }

    #[test]
    fn arithmetic_with_span_and_between_two_timestamps() {
        let e = engine();
        let src = r#"
            let t = parse_timestamp("2024-01-01T00:00:00Z");
            let later = t + days(1);
            let back = later - hours(24);
            [(later - t).whole_hours(), back == t]
        "#;
        let arr = e.eval::<rhai::Array>(src).unwrap();
        assert_eq!(arr[0].as_int().unwrap(), 24);
        assert!(arr[1].as_bool().unwrap());
    }

    #[test]
    fn comparisons_and_is_between() {
        let e = engine();
        let src = r#"
            let a = parse_timestamp("2024-01-01T00:00:00Z");
            let b = parse_timestamp("2024-06-01T00:00:00Z");
            let c = parse_timestamp("2024-12-01T00:00:00Z");
            [a < b, b <= c, c > a, b.is_between(a, c), a.is_between(b, c)]
        "#;
        let arr = e.eval::<rhai::Array>(src).unwrap();
        assert!(arr[0].as_bool().unwrap());
        assert!(arr[1].as_bool().unwrap());
        assert!(arr[2].as_bool().unwrap());
        assert!(arr[3].as_bool().unwrap());
        assert!(!arr[4].as_bool().unwrap());
    }

    #[test]
    fn an_invalid_format_string_is_a_catchable_error_not_a_panic() {
        let e = engine();
        let err = e
            .eval::<String>(r#"parse_timestamp("2024-01-01T00:00:00Z").format("%Q")"#)
            .unwrap_err();
        assert!(matches!(*err, rhai::EvalAltResult::ErrorRuntime(..)));
    }
}
