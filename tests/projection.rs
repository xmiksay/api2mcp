//! Black-box tests for `project::{CompiledProjection, apply}` against a realistic upstream-shaped
//! JSON body — `src/project/mod.rs` already carries focused unit tests per rule; this file is the
//! "does the whole thing hang together against something that looks like a real API response"
//! pass the chunk's definition of done asks for as its own `cargo test --test projection` target.

use api2mcp::model::{Cardinality, ParamType, Projection, ProjectionField};
use api2mcp::project::{CompiledProjection, apply};
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

fn upstream_body() -> serde_json::Value {
    json!({
        "user": {
            "id": "1042",
            "display_name": "Ada",
            "email": null
        },
        "repos": [
            {"name": "alpha", "stars": 12, "archived": false},
            {"name": "beta", "stars": "7", "archived": false},
            {"name": "gamma", "stars": 0, "archived": true}
        ],
        "rate_limit": {
            "remaining": 4999
        }
    })
}

#[test]
fn projects_scalar_and_collection_fields_from_a_realistic_body() {
    let projection = Projection {
        fields: vec![
            field("user_id", "$.user.id", Cardinality::One, None),
            field("email", "$.user.email", Cardinality::One, None),
            field("repo_names", "$.repos[*].name", Cardinality::Many, None),
            field(
                "star_counts",
                "$.repos[*].stars",
                Cardinality::Many,
                Some(ParamType::Integer),
            ),
        ],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid projection");
    let out = apply(&compiled, &upstream_body()).expect("projects");

    assert_eq!(out["user_id"], json!("1042"));
    // An explicit `null` in the body is a value, not an absence.
    assert_eq!(out["email"], serde_json::Value::Null);
    assert_eq!(out["repo_names"], json!(["alpha", "beta", "gamma"]));
    // Mixed number/digit-string upstream shapes both coerce cleanly to integers.
    assert_eq!(out["star_counts"], json!([12, 7, 0]));
}

#[test]
fn missing_field_under_cardinality_one_is_an_error() {
    let projection = Projection {
        fields: vec![field("nickname", "$.user.nickname", Cardinality::One, None)],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid");
    let err = apply(&compiled, &upstream_body()).unwrap_err();
    let json_err = serde_json::to_value(&err).expect("serializable");
    assert_eq!(json_err["error"], "missing");
}

#[test]
fn cardinality_one_against_a_multi_element_match_is_a_hard_error_never_first() {
    let projection = Projection {
        fields: vec![field(
            "a_repo_name",
            "$.repos[*].name",
            Cardinality::One,
            None,
        )],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid");
    let err = apply(&compiled, &upstream_body()).unwrap_err();
    let json_err = serde_json::to_value(&err).expect("serializable");
    assert_eq!(json_err["error"], "ambiguous_match");
    assert_eq!(json_err["count"], 3);
}

#[test]
fn cardinality_many_over_a_field_that_does_not_exist_at_all_yields_an_empty_array() {
    let projection = Projection {
        fields: vec![field(
            "labels",
            "$.repos[*].labels[*]",
            Cardinality::Many,
            None,
        )],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid");
    let out = apply(&compiled, &upstream_body()).expect("empty, not missing");
    assert_eq!(out["labels"], json!([]));
}

#[test]
fn type_mismatch_without_coercion_passes_the_raw_value_through() {
    let projection = Projection {
        fields: vec![field(
            "archived_flags",
            "$.repos[*].archived",
            Cardinality::Many,
            None,
        )],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid");
    let out = apply(&compiled, &upstream_body()).expect("ok, no coercion requested");
    assert_eq!(out["archived_flags"], json!([false, false, true]));
}

#[test]
fn type_mismatch_with_coercion_requested_is_a_typed_error() {
    let projection = Projection {
        fields: vec![field(
            "archived_flags",
            "$.repos[*].archived",
            Cardinality::Many,
            Some(ParamType::Integer),
        )],
    };
    let compiled = CompiledProjection::compile(&projection).expect("valid");
    let err = apply(&compiled, &upstream_body()).unwrap_err();
    let json_err = serde_json::to_value(&err).expect("serializable");
    assert_eq!(json_err["error"], "coercion_failed");
    assert_eq!(json_err["actual"], "boolean");
    assert_eq!(json_err["expected"], "integer");
}

#[test]
fn an_invalid_jsonpath_fails_to_compile_before_any_body_is_seen() {
    let projection = Projection {
        fields: vec![field("x", "$[", Cardinality::One, None)],
    };
    assert!(CompiledProjection::compile(&projection).is_err());
}
