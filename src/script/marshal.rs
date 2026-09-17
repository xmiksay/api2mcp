//! Hand-written `Dynamic`/`Map` <-> `serde_json::Value` marshalling — deliberately **not**
//! `rhai::serde`. `entanglement-runtime/src/script/data.rs` documents the caveat we diverge from:
//! `rhai::serde`'s bridge silently widens an integer outside `i64` range to an approximate
//! `FLOAT`, the same thing `JSON.parse` does. That is a correctness bug in an API gateway — a
//! Snowflake ID above `i64::MAX` must be refused, not rounded — so this module never delegates to
//! it.
//!
//! `serde_json::Map` and `rhai::Map` are both `BTreeMap`-backed (this crate never enables
//! `preserve_order`), so a round trip through this boundary sorts identically both ways; that's
//! the ordering guarantee I7 leans on. A JSON object's own *insertion* order is still destroyed by
//! the round trip — accepted crate-wide, not a bug here.

use rhai::{Array, Dynamic, Map};
use serde_json::{Map as JsonMap, Number, Value};
use thiserror::Error;

use super::dates::Timestamp;

/// Recursion bound in both directions. An attacker-shaped upstream response (or a script return
/// value built to match) with unbounded nesting is a stack overflow, which is not an error either
/// direction can report — refusing past a fixed depth is.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MarshalError {
    #[error("nesting exceeds the maximum depth of {MAX_DEPTH}")]
    TooDeep,
    /// `rhai::INT` is `i64` — a `u64` above `i64::MAX` (JSON has no separate integer/float
    /// syntax, so this can only arise from an actual whole-number literal too large for `i64`)
    /// has no lossless Rhai representation.
    #[error("integer {0} has no lossless Rhai representation (rhai::INT is i64)")]
    IntegerOutOfRange(u64),
    #[error("value of type {0} is not representable as JSON")]
    UnsupportedType(&'static str),
    #[error("float value is not finite")]
    NonFiniteFloat,
}

/// `serde_json::Value` -> `rhai::Dynamic`. `null` becomes `()`; a JSON object becomes a
/// [`Map`], sorted identically to the source (both `BTreeMap`-backed).
pub fn to_dynamic(value: &Value) -> Result<Dynamic, MarshalError> {
    to_dynamic_depth(value, 0)
}

fn to_dynamic_depth(value: &Value, depth: usize) -> Result<Dynamic, MarshalError> {
    if depth > MAX_DEPTH {
        return Err(MarshalError::TooDeep);
    }
    Ok(match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(*b),
        Value::Number(n) => number_to_dynamic(n)?,
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Array(items) => {
            let mut arr = Array::with_capacity(items.len());
            for item in items {
                arr.push(to_dynamic_depth(item, depth + 1)?);
            }
            Dynamic::from(arr)
        }
        Value::Object(obj) => {
            let mut map = Map::new();
            for (k, v) in obj {
                map.insert(k.as_str().into(), to_dynamic_depth(v, depth + 1)?);
            }
            Dynamic::from(map)
        }
    })
}

fn number_to_dynamic(n: &Number) -> Result<Dynamic, MarshalError> {
    if let Some(i) = n.as_i64() {
        return Ok(Dynamic::from(i));
    }
    if let Some(u) = n.as_u64() {
        // `as_i64` already failed, so this is necessarily above `i64::MAX` — the one case this
        // module refuses instead of approximating.
        return Err(MarshalError::IntegerOutOfRange(u));
    }
    let f = n.as_f64().ok_or(MarshalError::NonFiniteFloat)?;
    Ok(Dynamic::from(f))
}

/// `rhai::Dynamic` -> `serde_json::Value`. `()` becomes `null`; `Blob`/`FnPtr`/shared/custom
/// types are refused, naming the type rather than silently stringifying or dropping them.
pub fn to_json(value: &Dynamic) -> Result<Value, MarshalError> {
    to_json_depth(value, 0)
}

fn to_json_depth(value: &Dynamic, depth: usize) -> Result<Value, MarshalError> {
    if depth > MAX_DEPTH {
        return Err(MarshalError::TooDeep);
    }
    if value.is_unit() {
        return Ok(Value::Null);
    }
    if let Ok(b) = value.as_bool() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = value.as_int() {
        return Ok(Value::Number(i.into()));
    }
    if let Ok(f) = value.as_float() {
        return Number::from_f64(f)
            .map(Value::Number)
            .ok_or(MarshalError::NonFiniteFloat);
    }
    if value.is_string() {
        // `into_immutable_string` consumes; clone first so this stays a `&Dynamic` API.
        let s = value
            .clone()
            .into_immutable_string()
            .map_err(MarshalError::UnsupportedType)?;
        return Ok(Value::String(s.to_string()));
    }
    if value.is_array() {
        let arr = value.clone().cast::<Array>();
        let mut out = Vec::with_capacity(arr.len());
        for item in &arr {
            out.push(to_json_depth(item, depth + 1)?);
        }
        return Ok(Value::Array(out));
    }
    if value.is_map() {
        let map = value.clone().cast::<Map>();
        let mut out = JsonMap::new();
        for (k, v) in &map {
            out.insert(k.to_string(), to_json_depth(v, depth + 1)?);
        }
        return Ok(Value::Object(out));
    }
    // One-directional: a `Timestamp` returned from a script becomes an RFC 3339 JSON string, but
    // `to_dynamic` never does the reverse (a JSON string that merely *looks* date-shaped is never
    // auto-promoted to a `Timestamp`) — implicit type inference on an arbitrary incoming string
    // would be surprising, and could misfire on a string field that only coincidentally looks
    // like a date. Checked last, after every native rhai type above, since those checks are exact
    // and this one is a custom-type downcast.
    if let Some(ts) = value.clone().try_cast::<Timestamp>() {
        return Ok(Value::String(ts.to_rfc3339()));
    }
    Err(MarshalError::UnsupportedType(value.type_name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_round_trips_to_unit_and_back() {
        let d = to_dynamic(&Value::Null).expect("null marshals");
        assert!(d.is_unit());
        assert_eq!(to_json(&d).expect("unit marshals back"), Value::Null);
    }

    #[test]
    fn object_round_trips_through_both_btreemaps() {
        let original = json!({"b": 1, "a": [1, 2, "x"], "c": {"nested": true}});
        let d = to_dynamic(&original).expect("object marshals");
        let back = to_json(&d).expect("dynamic marshals back");
        assert_eq!(original, back);
    }

    #[test]
    fn i64_max_round_trips_exactly() {
        let original = json!(i64::MAX);
        let d = to_dynamic(&original).expect("i64::MAX marshals");
        assert_eq!(to_json(&d).unwrap(), original);
    }

    #[test]
    fn u64_above_i64_max_is_refused_not_approximated() {
        let over = (i64::MAX as u64) + 1;
        let err = to_dynamic(&json!(over)).unwrap_err();
        assert_eq!(err, MarshalError::IntegerOutOfRange(over));
    }

    #[test]
    fn u64_max_is_refused() {
        let err = to_dynamic(&json!(u64::MAX)).unwrap_err();
        assert_eq!(err, MarshalError::IntegerOutOfRange(u64::MAX));
    }

    #[test]
    fn depth_cap_is_enforced_on_the_way_in() {
        let mut value = json!(1);
        for _ in 0..(MAX_DEPTH + 10) {
            value = json!([value]);
        }
        assert_eq!(to_dynamic(&value).unwrap_err(), MarshalError::TooDeep);
    }

    #[test]
    fn depth_cap_is_enforced_on_the_way_out() {
        let arr: Array = vec![Dynamic::from(1_i64)];
        let mut d = Dynamic::from(arr);
        for _ in 0..(MAX_DEPTH + 10) {
            let outer: Array = vec![d];
            d = Dynamic::from(outer);
        }
        assert_eq!(to_json(&d).unwrap_err(), MarshalError::TooDeep);
    }

    #[test]
    fn blob_is_refused_naming_the_type() {
        let blob: rhai::Blob = vec![1u8, 2, 3];
        let d = Dynamic::from(blob);
        let err = to_json(&d).unwrap_err();
        assert!(matches!(err, MarshalError::UnsupportedType(_)));
    }

    #[test]
    fn timestamp_marshals_out_as_an_rfc3339_string_but_never_back_in() {
        let ts = Timestamp::from(
            chrono::DateTime::parse_from_rfc3339("2024-03-05T12:30:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        let json = to_json(&Dynamic::from(ts)).expect("Timestamp marshals to a JSON string");
        assert_eq!(json, Value::String("2024-03-05T12:30:00+00:00".to_owned()));

        // The reverse direction never promotes a date-shaped string back into a `Timestamp` — see
        // the comment at the `to_json_depth` call site.
        let back = to_dynamic(&json).expect("string marshals back in");
        assert!(
            back.is_string(),
            "must stay a plain string, not a Timestamp"
        );
    }

    #[test]
    fn preserve_order_is_never_enabled() {
        // Shared invariant this module leans on: both `Map` types sort identically only because
        // neither is `IndexMap`-backed.
        assert_eq!(
            serde_json::to_string(&json!({"b": 1, "a": 2})).unwrap(),
            r#"{"a":2,"b":1}"#
        );
    }
}
