//! Per-[`ParamType`] coercion. Deliberately narrow: coercion exists to accept the handful of JSON
//! shapes a well-behaved MCP client actually sends (a model that's been told a field is an
//! integer may still emit `"42"`), not to guess intent. `Boolean` never accepts truthiness —
//! `1`, `"yes"`, `"true"` are all rejected, not just non-canonical.

use serde_json::Value;

use crate::model::ParamType;

/// The JSON Schema type name of `value`, for error messages.
pub fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Coerces `value` to `ty`, returning the actual JSON type name on mismatch (the caller turns
/// that into a `ValidationError::WrongType`).
pub fn coerce(ty: ParamType, value: &Value) -> Result<Value, &'static str> {
    match ty {
        ParamType::String => coerce_string(value),
        ParamType::Integer => coerce_integer(value),
        ParamType::Number => coerce_number(value),
        ParamType::Boolean => coerce_boolean(value),
        ParamType::StringArray => coerce_string_array(value),
    }
}

fn coerce_string(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::String(_) => Ok(value.clone()),
        other => Err(json_type_name(other)),
    }
}

fn coerce_integer(value: &Value) -> Result<Value, &'static str> {
    match value {
        // An exact i64/u64 JSON number is already an integer.
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(value.clone()),
        // A JSON number with no fractional part (e.g. `3.0`) is still "an integer", just
        // spelled with a decimal point — normalise it to the integral form.
        Value::Number(n) => {
            let f = n.as_f64().ok_or("number")?;
            if f.fract() == 0.0 && f.is_finite() && (i64::MIN as f64..=i64::MAX as f64).contains(&f)
            {
                Ok(Value::Number((f as i64).into()))
            } else {
                Err("number")
            }
        }
        // A digit string (optionally signed), e.g. `"42"` or `"-7"` — never a float string.
        Value::String(s) => parse_digit_string(s)
            .map(|n| Value::Number(n.into()))
            .ok_or("string"),
        other => Err(json_type_name(other)),
    }
}

fn parse_digit_string(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn coerce_number(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::Number(_) => Ok(value.clone()),
        other => Err(json_type_name(other)),
    }
}

fn coerce_boolean(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::Bool(_) => Ok(value.clone()),
        other => Err(json_type_name(other)),
    }
}

fn coerce_string_array(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::Array(items) if items.iter().all(|v| matches!(v, Value::String(_))) => {
            Ok(value.clone())
        }
        Value::Array(_) => Err("array of non-strings"),
        other => Err(json_type_name(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn integer_accepts_exact_json_number() {
        assert_eq!(coerce(ParamType::Integer, &json!(42)).unwrap(), json!(42));
    }

    #[test]
    fn integer_accepts_whole_valued_float() {
        assert_eq!(coerce(ParamType::Integer, &json!(3.0)).unwrap(), json!(3));
    }

    #[test]
    fn integer_rejects_fractional_float() {
        assert!(coerce(ParamType::Integer, &json!(3.5)).is_err());
    }

    #[test]
    fn integer_accepts_digit_string() {
        assert_eq!(coerce(ParamType::Integer, &json!("42")).unwrap(), json!(42));
        assert_eq!(coerce(ParamType::Integer, &json!("-7")).unwrap(), json!(-7));
    }

    #[test]
    fn integer_rejects_non_digit_string() {
        assert!(coerce(ParamType::Integer, &json!("42.0")).is_err());
        assert!(coerce(ParamType::Integer, &json!("abc")).is_err());
        assert!(coerce(ParamType::Integer, &json!("")).is_err());
    }

    #[test]
    fn boolean_accepts_only_json_bool() {
        assert!(coerce(ParamType::Boolean, &json!(true)).is_ok());
        assert!(coerce(ParamType::Boolean, &json!(false)).is_ok());
    }

    #[test]
    fn boolean_rejects_truthiness_and_string_forms() {
        assert!(coerce(ParamType::Boolean, &json!(1)).is_err());
        assert!(coerce(ParamType::Boolean, &json!(0)).is_err());
        assert!(coerce(ParamType::Boolean, &json!("true")).is_err());
        assert!(coerce(ParamType::Boolean, &json!("yes")).is_err());
    }

    #[test]
    fn string_array_requires_all_string_elements() {
        assert!(coerce(ParamType::StringArray, &json!(["a", "b"])).is_ok());
        assert!(coerce(ParamType::StringArray, &json!(["a", 1])).is_err());
        assert!(coerce(ParamType::StringArray, &json!("a")).is_err());
    }

    #[test]
    fn wrong_type_reports_actual_json_type_name() {
        assert_eq!(coerce(ParamType::Integer, &json!(true)), Err("boolean"));
        assert_eq!(coerce(ParamType::String, &json!(1)), Err("number"));
    }
}
