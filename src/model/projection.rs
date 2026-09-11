//! The *description* of how to reshape an upstream response into what the model sees. Compiling
//! `path` into an executable `serde_json_path::JsonPath` and applying it happens in
//! [`crate::project`] at resolve time — this type is the pre-compile, DB-row form, kept as a raw
//! string for the same reason `ApiCall::path_template` is: this module has no business owning a
//! JSONPath compiler.

use super::param::ParamType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// Applied in this order; also the order fields appear in the projected output (I7).
    pub fields: Vec<ProjectionField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionField {
    pub name: String,
    /// RFC 9535 JSONPath expression, evaluated against the upstream response body.
    pub path: String,
    pub cardinality: Cardinality,
    /// Opt-in coercion of the matched JSON value(s). `None` passes the value through unchanged —
    /// design correction #6: the default is "don't lie about the data".
    pub coerce: Option<ParamType>,
}

/// Design correction #6: `One` matching more than one node is a hard error, not `first()` — a
/// silent `[0]` is exactly how a projection keeps "working" while returning the wrong thing after
/// an upstream shape change. `Many` matching zero nodes yields `[]`, never "missing": an empty
/// collection is a value, and `null` stays distinct from "no match".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Many,
}
