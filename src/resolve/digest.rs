//! `EndpointPlan::digest`: sha256 (hex) over the canonical JSON of every definition a plan was
//! built from. Deliberately built from the raw [`crate::model`] types, not the compiled
//! `resolve::plan` structures (`http::UrlTemplate`, `serde_json_path::JsonPath` — neither
//! implements `Serialize`) — the digest answers "did the *definition* change", and a
//! recompile of an unchanged definition must produce the same digest.
//!
//! Every collection walked here is a `BTreeMap`/iterated in a stable order (I7), and
//! `serde_json`'s `Map` is never built with `preserve_order` crate-wide, so
//! `serde_json::to_vec` on the snapshot below is byte-stable across runs.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{
    Access, ApiCall, Budgets, Cardinality, Pagination, Param, ParamLocation, ParamType, Projection,
    ScriptDef, Slug,
};
use crate::store::sha256_hex;

use super::plan::{PlannedApiCall, PlannedTool, ToolTarget};

pub fn compute(
    endpoint_slug: &Slug,
    write_ceiling: Access,
    endpoint_budgets: Budgets,
    calls: &BTreeMap<Slug, PlannedApiCall>,
    scripts: &BTreeMap<Slug, ScriptDef>,
    tools: &[PlannedTool],
) -> String {
    let snapshot = json!({
        "endpoint": endpoint_slug.as_str(),
        "write_ceiling": access_str(write_ceiling),
        "budgets": budgets_json(&endpoint_budgets),
        "calls": calls
            .iter()
            .map(|(slug, c)| (slug.as_str().to_owned(), api_call_json(&c.api_call)))
            .collect::<BTreeMap<_, _>>(),
        "scripts": scripts
            .iter()
            .map(|(slug, s)| (slug.as_str().to_owned(), script_json(s)))
            .collect::<BTreeMap<_, _>>(),
        "tools": tools
            .iter()
            .map(|t| (t.name.clone(), tool_json(t)))
            .collect::<BTreeMap<_, _>>(),
    });
    let canonical = serde_json::to_vec(&snapshot)
        .expect("snapshot is built entirely from json! over already-valid model values");
    sha256_hex(&canonical)
}

fn access_str(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "write",
    }
}

fn param_type_str(t: ParamType) -> &'static str {
    match t {
        ParamType::String => "string",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
        ParamType::StringArray => "string_array",
    }
}

fn param_location_json(l: &ParamLocation) -> Value {
    match l {
        ParamLocation::Path => json!("path"),
        ParamLocation::Query => json!("query"),
        ParamLocation::Header => json!("header"),
        ParamLocation::Body(pointer) => json!({"body": pointer.to_string()}),
        ParamLocation::Local => json!("local"),
    }
}

fn param_json(p: &Param) -> Value {
    json!({
        "name": p.name,
        "location": param_location_json(&p.location),
        "ty": param_type_str(p.ty),
        "required": p.required,
        "default": p.default,
        "fixed": p.fixed,
        "enum_values": p.enum_values,
        "position": p.position,
    })
}

fn pagination_json(p: &Pagination) -> Value {
    match p {
        Pagination::None => json!({"kind": "none"}),
        Pagination::Cursor {
            next_cursor_path,
            query_param,
        } => json!({
            "kind": "cursor",
            "next_cursor_path": next_cursor_path.to_string(),
            "query_param": query_param,
        }),
    }
}

fn projection_json(p: &Projection) -> Value {
    json!({
        "fields": p.fields.iter().map(|f| json!({
            "name": f.name,
            "path": f.path,
            "cardinality": match f.cardinality {
                Cardinality::One => "one",
                Cardinality::Many => "many",
            },
            "coerce": f.coerce.map(param_type_str),
        })).collect::<Vec<_>>(),
    })
}

fn api_call_json(c: &ApiCall) -> Value {
    json!({
        "slug": c.slug.as_str(),
        "service_slug": c.service_slug.as_str(),
        "auth_provider_slug": c.auth_provider_slug.as_ref().map(Slug::as_str),
        "method": c.method.as_str(),
        "path_template": c.path_template,
        "query_fixed": c.query_fixed,
        "body_template": c.body_template,
        "access": access_str(c.access),
        "idempotent": c.idempotent,
        "projection": c.projection.as_ref().map(projection_json),
        "pagination": pagination_json(&c.pagination),
        "timeout_ms": c.timeout_ms,
        "max_response_bytes": c.max_response_bytes,
        "params": c.params.iter().map(param_json).collect::<Vec<_>>(),
    })
}

fn script_json(s: &ScriptDef) -> Value {
    json!({
        "slug": s.slug.as_str(),
        "source": s.source,
        "params": s.params.iter().map(param_json).collect::<Vec<_>>(),
        "callable": s.callable
            .iter()
            .map(|(alias, target)| (alias.clone(), target.as_str().to_owned()))
            .collect::<BTreeMap<_, _>>(),
        "budgets": budgets_json(&s.budgets),
        "description": s.description,
    })
}

fn budgets_json(b: &Budgets) -> Value {
    json!({
        "max_calls": b.max_calls,
        "max_bytes": b.max_bytes,
        "wall_clock_ms": b.wall_clock.map(|d| d.as_millis() as u64),
        "max_pages": b.max_pages,
        "max_concurrency": b.max_concurrency,
    })
}

fn tool_json(t: &PlannedTool) -> Value {
    let target = match &t.target {
        ToolTarget::ApiCall(slug) => format!("api_call:{}", slug.as_str()),
        ToolTarget::Script(slug) => format!("script:{}", slug.as_str()),
    };
    json!({
        "target": target,
        "input_schema": t.input_schema,
        "budgets": budgets_json(&t.budgets),
    })
}
