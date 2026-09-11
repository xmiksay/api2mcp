//! MCP JSON Schema generation and caller-argument binding.
//!
//! The typed param list is the only source of truth; the generated schema is a *rendering*
//! of it. `bind_args` never trusts the schema it emitted — a caller that ignores the schema
//! is rejected by the same code that would have rejected a caller that read it.

pub(crate) mod coerce;
mod validate;

pub use validate::{ValidationError, bind_args};

use serde_json::{Map, Value};

use crate::model::{Param, ParamType};

/// Builds the MCP `inputSchema` for a param list: an `object` schema whose `properties` are the
/// model-visible params (`fixed` excluded — see [`Param::is_model_visible`]) and whose `required`
/// array lists the required ones, both walked in `position` order (I7) rather than declaration
/// order or (what `serde_json::Map`'s `BTreeMap` backing would otherwise impose) alphabetical
/// key order. `properties`' own serialized key order is unavoidably alphabetical — `serde_json`
/// is never built with `preserve_order` in this crate — so `position` order is only observable
/// via `required`'s array order; building `properties` by position is still correct, just not
/// independently visible once serialized.
pub fn input_schema(params: &[Param]) -> Value {
    let mut visible: Vec<&Param> = params.iter().filter(|p| p.is_model_visible()).collect();
    visible.sort_by_key(|p| p.position);

    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in visible {
        properties.insert(p.name.clone(), property_schema(p));
        if p.required {
            required.push(Value::String(p.name.clone()));
        }
    }

    let mut root = Map::new();
    root.insert("type".into(), Value::String("object".into()));
    root.insert("properties".into(), Value::Object(properties));
    root.insert("required".into(), Value::Array(required));
    Value::Object(root)
}

fn property_schema(p: &Param) -> Value {
    let mut obj = Map::new();
    insert_type(&mut obj, p.ty);
    if let Some(description) = &p.description {
        obj.insert("description".into(), Value::String(description.clone()));
    }
    if let Some(enum_values) = &p.enum_values {
        obj.insert("enum".into(), Value::Array(enum_values.clone()));
    }
    if let Some(default) = &p.default {
        obj.insert("default".into(), default.clone());
    }
    Value::Object(obj)
}

fn insert_type(obj: &mut Map<String, Value>, ty: ParamType) {
    obj.insert(
        "type".into(),
        Value::String(ty.json_schema_type_name().into()),
    );
    if matches!(ty, ParamType::StringArray) {
        obj.insert("items".into(), serde_json::json!({ "type": "string" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ParamLocation;
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
    fn fixed_param_absent_from_properties_and_required() {
        let mut secret = param("account_id", ParamType::String, true, 0);
        secret.fixed = Some(json!("trusted-account"));
        let visible = param("query", ParamType::String, true, 1);

        let schema = input_schema(&[secret, visible]);

        let properties = schema["properties"].as_object().expect("object");
        assert!(!properties.contains_key("account_id"));
        assert!(properties.contains_key("query"));

        let required = schema["required"].as_array().expect("array");
        assert!(!required.iter().any(|v| v == "account_id"));
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn required_order_follows_position_not_declaration_or_alphabetical_order() {
        // Declared "zebra" first, but its position (1) is after "apple"'s (0) — and alphabetical
        // order would put "apple" before "zebra" too, so pick names where position order and
        // alphabetical order actually disagree.
        let zebra = param("zebra", ParamType::String, true, 0);
        let apple = param("apple", ParamType::String, true, 1);

        let schema = input_schema(&[apple, zebra]);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("array")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();

        assert_eq!(required, vec!["zebra", "apple"]);
    }

    #[test]
    fn string_array_gets_items_schema() {
        let p = param("tags", ParamType::StringArray, false, 0);
        let schema = input_schema(&[p]);
        let prop = &schema["properties"]["tags"];
        assert_eq!(prop["type"], json!("array"));
        assert_eq!(prop["items"], json!({"type": "string"}));
    }

    #[test]
    fn description_enum_and_default_are_emitted() {
        let mut p = param("unit", ParamType::String, false, 0);
        p.description = Some("measurement unit".into());
        p.enum_values = Some(vec![json!("metric"), json!("imperial")]);
        p.default = Some(json!("metric"));

        let schema = input_schema(&[p]);
        let prop = &schema["properties"]["unit"];
        assert_eq!(prop["description"], json!("measurement unit"));
        assert_eq!(prop["enum"], json!(["metric", "imperial"]));
        assert_eq!(prop["default"], json!("metric"));
    }

    #[test]
    fn preserve_order_is_never_enabled() {
        // A regression guard for the crate-wide invariant this module leans on: serde_json's
        // Map must stay BTreeMap-backed, or `properties`' key order becomes insertion order
        // (IndexMap) instead of the alphabetical order this module's docs promise.
        assert_eq!(
            serde_json::to_string(&json!({"b": 1, "a": 2})).unwrap(),
            r#"{"a":2,"b":1}"#
        );
    }
}
