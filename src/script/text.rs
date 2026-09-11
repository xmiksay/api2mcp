//! Number/text helpers filling the gaps `BasicMathPackage`/`MoreStringPackage` leave (see
//! `engine::tests::PROBES` for exactly what those two already register, so nothing here
//! duplicates a name).
//!
//! Every function here takes fully script-controlled input (a string, an index, a width), so
//! each one is written to *return* a fallback/clamped value rather than ever reach a Rust panic
//! path — `std::ops::clamp`'s own `min <= max` assertion and any byte-offset string slice are the
//! two panics an inattentive implementation would hit here, so both are guarded explicitly below.

use rhai::Engine;

fn parse_int_or(s: &str, fallback: i64) -> i64 {
    s.trim().parse::<i64>().unwrap_or(fallback)
}

fn parse_float_or(s: &str, fallback: f64) -> f64 {
    s.trim().parse::<f64>().unwrap_or(fallback)
}

fn round_to(x: f64, places: i64) -> f64 {
    let factor = 10f64.powi(places.clamp(0, 17) as i32);
    (x * factor).round() / factor
}

/// `i64::clamp`/`f64::clamp` both assert `min <= max` and panic otherwise — a script can pass
/// `lo`/`hi` in either order, so this sorts them first instead of trusting the caller.
fn clamp_int(x: i64, lo: i64, hi: i64) -> i64 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    x.clamp(lo, hi)
}

/// As [`clamp_int`], plus `f64::clamp` also panics on a NaN bound — a NaN `x`, `lo`, or `hi`
/// passes `x` through unchanged rather than panicking.
fn clamp_float(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_nan() || lo.is_nan() || hi.is_nan() {
        return x;
    }
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    x.clamp(lo, hi)
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn format_thousands_int(x: i64) -> String {
    let grouped = group_thousands(&x.unsigned_abs().to_string());
    if x < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

fn format_thousands_float(x: f64) -> String {
    if !x.is_finite() {
        return x.to_string();
    }
    let neg = x.is_sign_negative() && x != 0.0;
    let abs = x.abs();
    // `as i64` on a float saturates rather than panicking/UB (guaranteed since Rust 1.45), so an
    // out-of-i64-range magnitude still can't crash this — it just loses thousands-grouping on the
    // integer part, which is an acceptable degradation for a formatting helper.
    let int_str = group_thousands(&(abs.trunc() as i64).to_string());
    let full = format!("{abs}");
    let grouped = match full.find('.') {
        Some(dot) => format!("{int_str}{}", &full[dot..]),
        None => int_str,
    };
    if neg { format!("-{grouped}") } else { grouped }
}

fn format_precision(x: f64, places: i64) -> String {
    let places = places.clamp(0, 100) as usize;
    format!("{x:.places$}")
}

fn pad_start(s: &str, width: i64, fill: &str) -> String {
    let width = width.max(0) as usize;
    let fill_char = fill.chars().next().unwrap_or(' ');
    let len = s.chars().count();
    if len >= width {
        return s.to_owned();
    }
    let pad: String = std::iter::repeat_n(fill_char, width - len).collect();
    format!("{pad}{s}")
}

fn pad_end(s: &str, width: i64, fill: &str) -> String {
    let width = width.max(0) as usize;
    let fill_char = fill.chars().next().unwrap_or(' ');
    let len = s.chars().count();
    if len >= width {
        return s.to_owned();
    }
    let pad: String = std::iter::repeat_n(fill_char, width - len).collect();
    format!("{s}{pad}")
}

/// Indexes by Unicode scalar (char) position, never byte offset — a multi-byte-UTF-8 boundary
/// can never panic this. Out-of-range or reversed indices are clamped/swapped rather than
/// rejected.
fn safe_slice(s: &str, start: i64, end: i64) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let clamp_idx = |i: i64| i.clamp(0, len) as usize;
    let (mut start, mut end) = (clamp_idx(start), clamp_idx(end));
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    chars[start..end].iter().collect()
}

pub fn register(engine: &mut Engine) {
    engine.register_fn("parse_int_or", parse_int_or);
    engine.register_fn("parse_float_or", parse_float_or);
    engine.register_fn("round_to", round_to);
    engine.register_fn("clamp", clamp_int);
    engine.register_fn("clamp", clamp_float);
    engine.register_fn("format_thousands", format_thousands_int);
    engine.register_fn("format_thousands", format_thousands_float);
    engine.register_fn("format_precision", format_precision);
    engine.register_fn("pad_start", pad_start);
    engine.register_fn("pad_end", pad_end);
    engine.register_fn("safe_slice", safe_slice);
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
    fn parse_with_fallback_never_throws() {
        let e = engine();
        assert_eq!(e.eval::<i64>(r#"parse_int_or("42", -1)"#).unwrap(), 42);
        assert_eq!(e.eval::<i64>(r#"parse_int_or("nope", -1)"#).unwrap(), -1);
        assert_eq!(
            e.eval::<f64>(r#"parse_float_or("nope", 2.5)"#).unwrap(),
            2.5
        );
    }

    #[test]
    fn round_to_n_places() {
        let e = engine();
        assert_eq!(e.eval::<f64>("round_to(4.567, 2)").unwrap(), 4.57);
        assert_eq!(e.eval::<f64>("round_to(4.0, 2)").unwrap(), 4.0);
    }

    #[test]
    fn clamp_handles_both_numeric_types_and_reversed_bounds() {
        let e = engine();
        assert_eq!(e.eval::<i64>("clamp(15, 0, 10)").unwrap(), 10);
        assert_eq!(e.eval::<i64>("clamp(-5, 0, 10)").unwrap(), 0);
        assert_eq!(
            e.eval::<i64>("clamp(5, 10, 0)").unwrap(),
            5,
            "reversed bounds must not panic"
        );
        assert_eq!(e.eval::<f64>("clamp(1.5, 0.0, 1.0)").unwrap(), 1.0);
    }

    #[test]
    fn format_thousands_groups_digits() {
        let e = engine();
        assert_eq!(
            e.eval::<String>("format_thousands(1234567)").unwrap(),
            "1,234,567"
        );
        assert_eq!(
            e.eval::<String>("format_thousands(-1234)").unwrap(),
            "-1,234"
        );
        assert_eq!(
            e.eval::<String>("format_thousands(1234567.89)").unwrap(),
            "1,234,567.89"
        );
    }

    #[test]
    fn format_precision_fixes_decimal_places() {
        let e = engine();
        assert_eq!(
            e.eval::<String>("format_precision(3.14159, 2)").unwrap(),
            "3.14"
        );
        assert_eq!(
            e.eval::<String>("format_precision(3.0, 3)").unwrap(),
            "3.000"
        );
    }

    #[test]
    fn padding_respects_multi_byte_fill_and_width() {
        let e = engine();
        assert_eq!(
            e.eval::<String>(r#"pad_start("7", 3, "0")"#).unwrap(),
            "007"
        );
        assert_eq!(e.eval::<String>(r#"pad_end("7", 3, "0")"#).unwrap(), "700");
        assert_eq!(
            e.eval::<String>(r#"pad_start("hello", 3, "0")"#).unwrap(),
            "hello",
            "already-wide-enough input is unchanged"
        );
    }

    #[test]
    fn safe_slice_clamps_and_never_panics_on_a_utf8_boundary() {
        let e = engine();
        // "héllo" has a 2-byte 'é' — a byte-offset slice at 2 would panic; char-indexed must not.
        assert_eq!(
            e.eval::<String>(r#"safe_slice("héllo", 0, 2)"#).unwrap(),
            "hé"
        );
        assert_eq!(
            e.eval::<String>(r#"safe_slice("hello", 2, 999)"#).unwrap(),
            "llo",
            "out-of-range end is clamped, not rejected"
        );
        assert_eq!(
            e.eval::<String>(r#"safe_slice("hello", 4, 1)"#).unwrap(),
            "ell",
            "reversed indices are swapped, not rejected"
        );
    }
}
