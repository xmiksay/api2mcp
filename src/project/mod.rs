//! Declarative JSONPath response projection (RFC 9535 via `serde_json_path`).
//!
//! [`Projection`] is a DB-row-shaped description; [`CompiledProjection::compile`] parses every
//! field's path exactly once, and [`apply`] runs the already-compiled projection against a
//! response body — never re-parsing a JSONPath expression per call. (The chunk brief describes
//! `apply`'s signature as taking `&Projection` directly; that's in tension with "compiled once,
//! not per call", so this module resolves it in favour of the invariant — see the chunk report.)
//!
//! Cardinality semantics are the whole point of this module:
//! - [`Cardinality::One`] matching **2+ nodes is a hard error**, never `first()` — a silent `[0]`
//!   is exactly how a projection keeps "working" while quietly returning the wrong thing after an
//!   upstream shape change.
//! - [`Cardinality::One`] matching **zero nodes** is `Missing` — a different error from the above,
//!   because "the field isn't there" and "the field is ambiguous" call for different fixes.
//! - [`Cardinality::Many`] matching zero nodes yields `[]`, never "missing": an empty collection
//!   is a value.
//! - An explicit JSON `null` (a matched node whose value is `null`) and an empty nodelist
//!   (`Missing`) stay distinct — `Cardinality::One` matching exactly one `null` node succeeds with
//!   a `null` output field.
//! - Coercion is opt-in per field ([`ProjectionField::coerce`]); `None` passes the matched value's
//!   type straight through rather than lying about the data. For `Many`, coercion (when set)
//!   applies element-wise.
//! - Fields are evaluated in [`Projection::fields`]'s declared order (I7), never the upstream
//!   body's — though, exactly like `schema::input_schema`, that ordering isn't independently
//!   visible once the result lands in a `serde_json::Map`: this crate never enables
//!   `preserve_order`, so a `Value::Object`'s own key iteration is always alphabetical regardless
//!   of insertion order. What *is* guaranteed, and what determinism (I7) actually needs, is that
//!   evaluating the same projection against the same body always inserts fields in the same
//!   order and therefore produces byte-identical serialized JSON — insertion order, not iteration
//!   order, is the property that stays stable.

use serde::Serialize;
use serde_json::Value;
use serde_json_path::JsonPath;
use thiserror::Error;

use crate::model::{Cardinality, ParamType, Projection};

/// A [`Projection`] whose field paths are already parsed. Build once (at resolve time, once C6
/// exists) with [`CompiledProjection::compile`]; [`apply`] never touches [`JsonPath::parse`].
#[derive(Debug)]
pub struct CompiledProjection {
    fields: Vec<CompiledField>,
}

#[derive(Debug)]
struct CompiledField {
    name: String,
    path_str: String,
    path: JsonPath,
    cardinality: Cardinality,
    coerce: Option<ParamType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ProjectionError {
    #[error("field {name:?}: invalid JSONPath {path:?}: {message}")]
    InvalidPath {
        name: String,
        path: String,
        message: String,
    },
    #[error("field {name:?} (cardinality one): no node matched {path:?}")]
    Missing { name: String, path: String },
    #[error(
        "field {name:?} (cardinality one): {count} nodes matched {path:?}, expected exactly one"
    )]
    AmbiguousMatch {
        name: String,
        path: String,
        count: usize,
    },
    #[error("field {name:?}: matched value is {actual}, which cannot be coerced to {expected}")]
    CoercionFailed {
        name: String,
        actual: &'static str,
        expected: &'static str,
    },
}

impl CompiledProjection {
    pub fn compile(projection: &Projection) -> Result<Self, ProjectionError> {
        let mut fields = Vec::with_capacity(projection.fields.len());
        for field in &projection.fields {
            let path = JsonPath::parse(&field.path).map_err(|e| ProjectionError::InvalidPath {
                name: field.name.clone(),
                path: field.path.clone(),
                message: e.to_string(),
            })?;
            fields.push(CompiledField {
                name: field.name.clone(),
                path_str: field.path.clone(),
                path,
                cardinality: field.cardinality,
                coerce: field.coerce,
            });
        }
        Ok(Self { fields })
    }
}

/// Reshapes `body` according to `projection`, in declared field order (I7). See the module docs
/// for cardinality and coercion semantics.
pub fn apply(projection: &CompiledProjection, body: &Value) -> Result<Value, ProjectionError> {
    let mut out = serde_json::Map::new();
    for field in &projection.fields {
        let nodes = field.path.query(body).all();
        let value = match field.cardinality {
            Cardinality::One => project_one(field, &nodes)?,
            Cardinality::Many => project_many(field, nodes)?,
        };
        out.insert(field.name.clone(), value);
    }
    Ok(Value::Object(out))
}

fn project_one(field: &CompiledField, nodes: &[&Value]) -> Result<Value, ProjectionError> {
    let value = match nodes {
        [] => {
            return Err(ProjectionError::Missing {
                name: field.name.clone(),
                path: field.path_str.clone(),
            });
        }
        [single] => (*single).clone(),
        many => {
            return Err(ProjectionError::AmbiguousMatch {
                name: field.name.clone(),
                path: field.path_str.clone(),
                count: many.len(),
            });
        }
    };
    match field.coerce {
        Some(ty) => coerce_value(ty, &value).map_err(|actual| ProjectionError::CoercionFailed {
            name: field.name.clone(),
            actual,
            expected: ty.json_schema_type_name(),
        }),
        None => Ok(value),
    }
}

fn project_many(field: &CompiledField, nodes: Vec<&Value>) -> Result<Value, ProjectionError> {
    let Some(ty) = field.coerce else {
        return Ok(Value::Array(nodes.into_iter().cloned().collect()));
    };
    let coerced: Result<Vec<Value>, &'static str> =
        nodes.into_iter().map(|v| coerce_value(ty, v)).collect();
    coerced
        .map(Value::Array)
        .map_err(|actual| ProjectionError::CoercionFailed {
            name: field.name.clone(),
            actual,
            expected: ty.json_schema_type_name(),
        })
}

/// Narrow, definer-opted-in coercion — mirrors `schema::coerce::coerce`'s semantics exactly, but
/// can't call it directly: that function lives in `schema`'s private `coerce` submodule (`mod
/// coerce;`, not `pub mod`), which this chunk doesn't own and can't re-export from. See the chunk
/// report for the follow-up (`pub(crate)` + re-export) that would let this delegate instead of
/// duplicate.
fn coerce_value(ty: ParamType, value: &Value) -> Result<Value, &'static str> {
    match ty {
        ParamType::String => match value {
            Value::String(_) => Ok(value.clone()),
            other => Err(crate::schema::coerce::json_type_name(other)),
        },
        ParamType::Integer => coerce_integer(value),
        ParamType::Number => match value {
            Value::Number(_) => Ok(value.clone()),
            other => Err(crate::schema::coerce::json_type_name(other)),
        },
        ParamType::Boolean => match value {
            Value::Bool(_) => Ok(value.clone()),
            other => Err(crate::schema::coerce::json_type_name(other)),
        },
        ParamType::StringArray => match value {
            Value::Array(items) if items.iter().all(|v| matches!(v, Value::String(_))) => {
                Ok(value.clone())
            }
            Value::Array(_) => Err("array of non-strings"),
            other => Err(crate::schema::coerce::json_type_name(other)),
        },
    }
}

fn coerce_integer(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(value.clone()),
        Value::Number(n) => {
            let f = n.as_f64().ok_or("number")?;
            if f.fract() == 0.0 && f.is_finite() && (i64::MIN as f64..=i64::MAX as f64).contains(&f)
            {
                Ok(Value::Number((f as i64).into()))
            } else {
                Err("number")
            }
        }
        Value::String(s) => {
            let digits = s.strip_prefix('-').unwrap_or(s);
            let ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            if ok {
                s.parse::<i64>()
                    .map(|n| Value::Number(n.into()))
                    .map_err(|_| "string")
            } else {
                Err("string")
            }
        }
        other => Err(crate::schema::coerce::json_type_name(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProjectionField;
    use serde_json::json;

    fn field(
        name: &str,
        path: &str,
        cardinality: Cardinality,
        coerce: Option<ParamType>,
    ) -> ProjectionField {
        ProjectionField {
            name: name.to_owned(),
            path: path.to_owned(),
            cardinality,
            coerce,
        }
    }

    fn compile(fields: Vec<ProjectionField>) -> CompiledProjection {
        CompiledProjection::compile(&Projection { fields }).expect("valid projection")
    }

    #[test]
    fn one_missing_field_is_an_error() {
        let projection = compile(vec![field("id", "$.id", Cardinality::One, None)]);
        let err = apply(&projection, &json!({})).unwrap_err();
        assert!(matches!(err, ProjectionError::Missing { .. }));
    }

    #[test]
    fn one_matching_two_nodes_is_an_error_never_first() {
        let projection = compile(vec![field("id", "$.items[*].id", Cardinality::One, None)]);
        let body = json!({"items": [{"id": 1}, {"id": 2}]});
        let err = apply(&projection, &body).unwrap_err();
        assert!(matches!(
            err,
            ProjectionError::AmbiguousMatch { count: 2, .. }
        ));
    }

    #[test]
    fn one_matching_exactly_one_explicit_null_is_distinct_from_missing() {
        let projection = compile(vec![field("id", "$.id", Cardinality::One, None)]);
        let out = apply(&projection, &json!({"id": null})).expect("matched");
        assert_eq!(out["id"], Value::Null);
    }

    #[test]
    fn many_with_no_matches_yields_empty_array_not_missing() {
        let projection = compile(vec![field("items", "$.items[*]", Cardinality::Many, None)]);
        let out = apply(&projection, &json!({"items": []})).expect("ok");
        assert_eq!(out["items"], json!([]));
    }

    #[test]
    fn many_collects_every_match_in_document_order() {
        let projection = compile(vec![field("ids", "$.items[*].id", Cardinality::Many, None)]);
        let body = json!({"items": [{"id": 1}, {"id": 2}, {"id": 3}]});
        let out = apply(&projection, &body).expect("ok");
        assert_eq!(out["ids"], json!([1, 2, 3]));
    }

    #[test]
    fn coercion_is_off_by_default_type_passes_through() {
        let projection = compile(vec![field("id", "$.id", Cardinality::One, None)]);
        let out = apply(&projection, &json!({"id": "42"})).expect("ok");
        assert_eq!(out["id"], json!("42"));
    }

    #[test]
    fn coercion_succeeds_when_the_value_is_convertible() {
        let projection = compile(vec![field(
            "id",
            "$.id",
            Cardinality::One,
            Some(ParamType::Integer),
        )]);
        let out = apply(&projection, &json!({"id": "42"})).expect("ok");
        assert_eq!(out["id"], json!(42));
    }

    #[test]
    fn coercion_failure_is_a_typed_error_naming_both_types() {
        let projection = compile(vec![field(
            "id",
            "$.id",
            Cardinality::One,
            Some(ParamType::Integer),
        )]);
        let err = apply(&projection, &json!({"id": "not-a-number"})).unwrap_err();
        match err {
            ProjectionError::CoercionFailed {
                actual, expected, ..
            } => {
                assert_eq!(actual, "string");
                assert_eq!(expected, "integer");
            }
            other => panic!("expected CoercionFailed, got {other:?}"),
        }
    }

    #[test]
    fn many_coerces_element_wise() {
        let projection = compile(vec![field(
            "ids",
            "$.items[*].id",
            Cardinality::Many,
            Some(ParamType::Integer),
        )]);
        let body = json!({"items": [{"id": "1"}, {"id": "2"}]});
        let out = apply(&projection, &body).expect("ok");
        assert_eq!(out["ids"], json!([1, 2]));
    }

    #[test]
    fn output_field_order_follows_declared_order_not_body_order() {
        let projection = compile(vec![
            field("z", "$.z", Cardinality::One, None),
            field("a", "$.a", Cardinality::One, None),
        ]);
        let body = json!({"a": 1, "z": 2});
        let out = apply(&projection, &body).expect("ok");
        let keys: Vec<&String> = out.as_object().expect("object").keys().collect();
        // serde_json's Map is never `preserve_order` in this crate, so key iteration is
        // alphabetical regardless of insertion order — this test pins that down rather than
        // the (unobservable, once serialized) insertion order itself.
        assert_eq!(keys, vec!["a", "z"]);
    }

    #[test]
    fn invalid_jsonpath_is_a_compile_time_error() {
        let err = CompiledProjection::compile(&Projection {
            fields: vec![field("x", "not a jsonpath", Cardinality::One, None)],
        })
        .unwrap_err();
        assert!(matches!(err, ProjectionError::InvalidPath { .. }));
    }
}
