//! Caller-argument binding. `bind_args` is the single gate a tool call's arguments pass through —
//! it never trusts the schema `input_schema` emitted for the same param list, so a caller who
//! ignores the generated schema entirely is rejected by exactly the same checks as one who read
//! it and tried to cheat it.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::model::Param;

use super::coerce;

/// Every variant names the offending param — these reach the model as tool-call errors, so a
/// `String` context (which could smuggle anything, including upstream response fragments) is not
/// good enough here. `thiserror` + `Serialize`, never `anyhow`, below this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ValidationError {
    #[error("arguments must be a JSON object")]
    NotAnObject,

    #[error("unknown parameter {param:?}")]
    UnknownParam { param: String },

    #[error("missing required parameter {param:?}")]
    MissingRequired { param: String },

    #[error("parameter {param:?}: expected {expected}, got {got}")]
    WrongType {
        param: String,
        expected: &'static str,
        got: &'static str,
    },

    #[error("parameter {param:?} is not one of the allowed values")]
    NotInEnum { param: String },
}

/// Type-checks/coerces every argument against `params`, applies defaults for absent optional
/// params, injects `fixed` values regardless of what the caller sent, and rejects any key not
/// named by `params`. The returned map holds only params that ended up with a value — an absent
/// optional param with no default is simply not a key in the result.
pub fn bind_args(
    params: &[Param],
    args: &Value,
) -> Result<BTreeMap<String, Value>, ValidationError> {
    let obj = args.as_object().ok_or(ValidationError::NotAnObject)?;

    let mut bound = BTreeMap::new();
    let mut known: BTreeSet<&str> = BTreeSet::new();

    for p in params {
        known.insert(p.name.as_str());

        // A fixed param is set by the definer and is not model-visible (schema::input_schema
        // omits it) — so it is never read from the caller's input, even if present there.
        if let Some(fixed) = &p.fixed {
            bound.insert(p.name.clone(), fixed.clone());
            continue;
        }

        match obj.get(&p.name) {
            Some(value) => {
                let coerced =
                    coerce::coerce(p.ty, value).map_err(|got| ValidationError::WrongType {
                        param: p.name.clone(),
                        expected: p.ty.json_schema_type_name(),
                        got,
                    })?;
                if let Some(enum_values) = &p.enum_values
                    && !enum_values.contains(&coerced)
                {
                    return Err(ValidationError::NotInEnum {
                        param: p.name.clone(),
                    });
                }
                bound.insert(p.name.clone(), coerced);
            }
            None => {
                if let Some(default) = &p.default {
                    bound.insert(p.name.clone(), default.clone());
                } else if p.required {
                    return Err(ValidationError::MissingRequired {
                        param: p.name.clone(),
                    });
                }
            }
        }
    }

    if let Some(unknown) = obj.keys().find(|k| !known.contains(k.as_str())) {
        return Err(ValidationError::UnknownParam {
            param: unknown.clone(),
        });
    }

    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ParamLocation, ParamType};
    use serde_json::json;

    fn param(name: &str, ty: ParamType, required: bool, position: i32) -> Param {
        Param {
            name: name.to_owned(),
            location: ParamLocation::Query,
            ty,
            required,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position,
        }
    }

    #[test]
    fn rejects_wrong_type() {
        let params = vec![param("limit", ParamType::Integer, true, 0)];
        let err = bind_args(&params, &json!({"limit": "abc"})).unwrap_err();
        assert_eq!(
            err,
            ValidationError::WrongType {
                param: "limit".into(),
                expected: "integer",
                got: "string"
            }
        );
    }

    #[test]
    fn rejects_unknown_key() {
        let params = vec![param("q", ParamType::String, false, 0)];
        let err = bind_args(&params, &json!({"q": "x", "bogus": 1})).unwrap_err();
        assert_eq!(
            err,
            ValidationError::UnknownParam {
                param: "bogus".into()
            }
        );
    }

    #[test]
    fn rejects_missing_required() {
        let params = vec![param("q", ParamType::String, true, 0)];
        let err = bind_args(&params, &json!({})).unwrap_err();
        assert_eq!(err, ValidationError::MissingRequired { param: "q".into() });
    }

    #[test]
    fn applies_default_when_absent() {
        let mut p = param("limit", ParamType::Integer, false, 0);
        p.default = Some(json!(20));
        let bound = bind_args(&[p], &json!({})).unwrap();
        assert_eq!(bound.get("limit"), Some(&json!(20)));
    }

    #[test]
    fn injects_fixed_even_over_a_conflicting_caller_value() {
        let mut p = param("account_id", ParamType::String, false, 0);
        p.fixed = Some(json!("trusted-account"));
        let bound = bind_args(&[p], &json!({"account_id": "attacker-account"})).unwrap();
        assert_eq!(bound.get("account_id"), Some(&json!("trusted-account")));
    }

    #[test]
    fn fixed_param_name_is_not_unknown() {
        let mut p = param("account_id", ParamType::String, false, 0);
        p.fixed = Some(json!("trusted-account"));
        // The caller supplying the fixed param's name must not trip UnknownParam, even though
        // that name never appears in the generated schema.
        assert!(bind_args(&[p], &json!({"account_id": "whatever"})).is_ok());
    }

    #[test]
    fn rejects_value_outside_enum() {
        let mut p = param("unit", ParamType::String, true, 0);
        p.enum_values = Some(vec![json!("metric"), json!("imperial")]);
        let err = bind_args(&[p], &json!({"unit": "furlongs"})).unwrap_err();
        assert_eq!(
            err,
            ValidationError::NotInEnum {
                param: "unit".into()
            }
        );
    }

    #[test]
    fn rejects_non_object_args() {
        let params = vec![param("q", ParamType::String, false, 0)];
        assert_eq!(
            bind_args(&params, &json!("not an object")).unwrap_err(),
            ValidationError::NotAnObject
        );
    }

    #[test]
    fn optional_param_absent_without_default_is_simply_omitted() {
        let params = vec![param("q", ParamType::String, false, 0)];
        let bound = bind_args(&params, &json!({})).unwrap();
        assert!(!bound.contains_key("q"));
    }
}
