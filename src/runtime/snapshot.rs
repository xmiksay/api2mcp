//! Builds `runs.definition_snapshot`: the complete compiled slice a tool ran from — the api_call
//! or script, its params, its projection, the service minus credentials, and the folded budgets.
//! With I8 (versioned definitions) deliberately not implemented, this blob is the *only* answer
//! to "what did this tool look like when it ran", so an incomplete one silently destroys the
//! audit story — see the module's own tests for what's asserted present.
//!
//! Deliberately its own, self-contained set of JSON builders rather than reusing
//! `resolve::digest`'s (which build near-identical shapes): that module is private to `resolve/`
//! (`mod digest;`, not `pub`), so nothing outside it can call in. Hoisting a shared,
//! `pub(crate)` set of these helpers would remove the duplication; until then the two shape-
//! builders have to be kept in sync by hand.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{
    Access, ApiCall, Budgets, Cardinality, Pagination, Param, ParamLocation, ParamType, Projection,
    ScriptDef, Service,
};
use crate::resolve::EndpointPlan;
use crate::resolve::plan::{PlannedTool, ToolTarget};

/// The full definition slice a tool ran from, redaction-safe by construction: nothing here ever
/// touches a credential value — `Service`/`ApiCall` structurally can't hold one (I4), and an
/// api_call names no auth provider at all (a service has at most one, applied by
/// `runtime::dispatch` from the service slug, never recorded on the call itself).
pub fn build_snapshot(plan: &EndpointPlan, tool: &PlannedTool) -> Value {
    match &tool.target {
        ToolTarget::ApiCall(slug) => {
            let planned = plan.calls.get(slug);
            json!({
                "kind": "api_call",
                "target_slug": slug.as_str(),
                "budgets": budgets_json(&tool.budgets),
                "api_call": planned.map(|p| api_call_json(&p.api_call)),
                "service": planned.map(|p| service_json(&p.service)),
            })
        }
        ToolTarget::Script(slug) => {
            let script = plan.scripts.get(slug);
            let reachable = plan.callable_by.get(slug).cloned().unwrap_or_default();
            let callable: BTreeMap<String, Value> = reachable
                .iter()
                .filter_map(|(alias, call_slug)| {
                    plan.calls.get(call_slug).map(|p| {
                        (
                            alias.clone(),
                            json!({
                                "api_call": api_call_json(&p.api_call),
                                "service": service_json(&p.service),
                            }),
                        )
                    })
                })
                .collect();
            json!({
                "kind": "script",
                "target_slug": slug.as_str(),
                "budgets": budgets_json(&tool.budgets),
                "script": script.map(script_json),
                "callable": callable,
            })
        }
    }
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

/// `Service` structurally can't carry a credential (I4) — `credential_env_key` lives on
/// `AuthProvider`, not here — so serializing every field verbatim is safe.
fn service_json(s: &Service) -> Value {
    json!({
        "slug": s.slug.as_str(),
        "base_url": s.base_url.as_str(),
        "origin_allowlist": s.origin_allowlist.iter().map(|o| o.to_string()).collect::<Vec<_>>(),
        "default_headers": s.default_headers,
        "timeout_ms": s.timeout_ms,
        "max_concurrency": s.max_concurrency,
        "rate_limit_per_min": s.rate_limit_per_min,
        "max_response_bytes": s.max_response_bytes,
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::http::UrlTemplate;
    use crate::model::{Origin, Slug};
    use crate::resolve::plan::PlannedApiCall;

    fn service() -> Service {
        let base_url: url::Url = "https://svc.example.com/".parse().unwrap();
        Service {
            owner_id: uuid::Uuid::nil(),
            slug: "svc".parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn api_call() -> ApiCall {
        ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: "call-a".parse().unwrap(),
            service_slug: "svc".parse().unwrap(),
            method: http::Method::GET,
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: Access::Read,
            idempotent: true,
            projection: None,
            pagination: Pagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![],
            description: None,
        }
    }

    fn plan_with_api_call() -> (EndpointPlan, PlannedTool) {
        let svc = service();
        let call = api_call();
        let planned = PlannedApiCall {
            url_template: UrlTemplate::parse(&call.path_template).unwrap(),
            origin: Origin::of(&svc.base_url).unwrap(),
            api_call: call.clone(),
            service: svc,
            projection: None,
        };
        let mut calls = BTreeMap::new();
        calls.insert(call.slug.clone(), planned);
        let tool = PlannedTool {
            name: "call-a".to_owned(),
            input_schema: json!({}),
            target: ToolTarget::ApiCall(call.slug.clone()),
            budgets: Budgets::default(),
        };
        let plan = EndpointPlan {
            owner_id: uuid::Uuid::nil(),
            slug: "ep".parse().unwrap(),
            write_ceiling: Access::Read,
            instructions: None,
            tools: vec![tool.clone()],
            calls,
            scripts: BTreeMap::new(),
            callable_by: BTreeMap::new(),
            origins: BTreeSet::new(),
            budgets: Budgets::default(),
            digest: "digest".to_owned(),
        };
        (plan, tool)
    }

    #[test]
    fn api_call_snapshot_carries_the_full_compiled_slice() {
        let (plan, tool) = plan_with_api_call();
        let snapshot = build_snapshot(&plan, &tool);
        assert_eq!(snapshot["kind"], json!("api_call"));
        assert_eq!(snapshot["api_call"]["slug"], json!("call-a"));
        assert_eq!(snapshot["service"]["slug"], json!("svc"));
        assert!(snapshot["budgets"].is_object());
    }

    #[test]
    fn no_field_in_a_service_snapshot_can_ever_be_a_credential_value() {
        // Structural check: `service_json` only ever reads fields `Service` actually has, and
        // that type has no credential-shaped field to begin with (I4) — this test pins the
        // *set* of keys emitted, so a future field addition to `Service` can't silently start
        // leaking through here unnoticed.
        let snapshot = service_json(&service());
        let keys: BTreeSet<&str> = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "slug",
                "base_url",
                "origin_allowlist",
                "default_headers",
                "timeout_ms",
                "max_concurrency",
                "rate_limit_per_min",
                "max_response_bytes",
            ])
        );
    }

    #[test]
    fn script_snapshot_includes_reachable_api_calls_keyed_by_alias() {
        let svc = service();
        let call = api_call();
        let planned = PlannedApiCall {
            url_template: UrlTemplate::parse(&call.path_template).unwrap(),
            origin: Origin::of(&svc.base_url).unwrap(),
            api_call: call.clone(),
            service: svc,
            projection: None,
        };
        let mut calls = BTreeMap::new();
        calls.insert(call.slug.clone(), planned);

        let script_slug: Slug = "script-a".parse().unwrap();
        let script = ScriptDef {
            owner_id: uuid::Uuid::nil(),
            slug: script_slug.clone(),
            source: "()".to_owned(),
            params: vec![],
            callable: BTreeMap::from([("helper".to_owned(), call.slug.clone())]),
            budgets: Budgets::default(),
            description: None,
        };
        let mut scripts = BTreeMap::new();
        scripts.insert(script_slug.clone(), script);
        let mut callable_by = BTreeMap::new();
        callable_by.insert(
            script_slug.clone(),
            BTreeMap::from([("helper".to_owned(), call.slug.clone())]),
        );

        let tool = PlannedTool {
            name: "script-a".to_owned(),
            input_schema: json!({}),
            target: ToolTarget::Script(script_slug.clone()),
            budgets: Budgets::default(),
        };
        let plan = EndpointPlan {
            owner_id: uuid::Uuid::nil(),
            slug: "ep".parse().unwrap(),
            write_ceiling: Access::Read,
            instructions: None,
            tools: vec![tool.clone()],
            calls,
            scripts,
            callable_by,
            origins: BTreeSet::new(),
            budgets: Budgets::default(),
            digest: "digest".to_owned(),
        };

        let snapshot = build_snapshot(&plan, &tool);
        assert_eq!(snapshot["kind"], json!("script"));
        assert_eq!(snapshot["script"]["slug"], json!("script-a"));
        assert_eq!(
            snapshot["callable"]["helper"]["api_call"]["slug"],
            json!("call-a")
        );
    }
}
