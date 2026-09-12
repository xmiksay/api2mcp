//! Turns caller-bound arguments into a fully-assembled, containment-checked request — the pure
//! half of "send a request" ([`crate::http::send`] owns actually sending it).
//!
//! `args` is expected to already be the output of `schema::bind_args`: type-checked, defaults
//! applied, `fixed` values injected, unknown keys rejected. `bind` does not re-validate types —
//! it routes already-validated values to the right part of the request and re-checks the
//! shape constraints (no arrays in a path/header slot, no CR/LF in a header value)
//! that `bind_args` has no reason to know about.

use std::collections::BTreeMap;

use jsonptr::PointerBuf;
use serde_json::Value;
use url::Host;

use crate::model::{ApiCall, Param, ParamLocation, Service, is_header_param_name_allowed};

use super::BindError;
use super::url_template::UrlTemplate;

/// An assembled, ready-to-send request. Carries no credential — [`crate::http::send`] applies auth
/// separately, after this is built, so this type can be constructed and asserted against in a
/// unit test with no [`crate::secret::Secret`] in sight.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundRequest {
    pub method: ::http::Method,
    pub url: url::Url,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
}

/// Binds `args` (already validated by `schema::bind_args`) into a request for `call` against
/// `service`. See the module docs for what's already guaranteed by the time this runs.
pub fn bind(
    call: &ApiCall,
    service: &Service,
    args: &BTreeMap<String, Value>,
) -> Result<BoundRequest, BindError> {
    let template = UrlTemplate::parse(&call.path_template)?;

    let mut path_values: BTreeMap<String, String> = BTreeMap::new();
    let mut query_values: BTreeMap<String, Vec<String>> = call
        .query_fixed
        .iter()
        .map(|(k, v)| (k.clone(), vec![v.clone()]))
        .collect();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut body = call.body_template.clone();

    for param in &call.params {
        let Some(value) = args.get(&param.name) else {
            continue;
        };
        bind_one(
            param,
            value,
            &mut path_values,
            &mut query_values,
            &mut headers,
            &mut body,
        )?;
    }

    let path = template.render(&path_values)?;
    let query = encode_query(&query_values);
    let url = assemble_url(service, &path, &query)?;

    Ok(BoundRequest {
        method: call.method.clone(),
        url,
        headers,
        body,
    })
}

fn bind_one(
    param: &Param,
    value: &Value,
    path_values: &mut BTreeMap<String, String>,
    query_values: &mut BTreeMap<String, Vec<String>>,
    headers: &mut BTreeMap<String, String>,
    body: &mut Option<Value>,
) -> Result<(), BindError> {
    match &param.location {
        ParamLocation::Path => {
            path_values.insert(param.name.clone(), scalar_string(&param.name, value)?);
        }
        ParamLocation::Query => {
            query_values.insert(param.name.clone(), query_strings(&param.name, value)?);
        }
        ParamLocation::Header => {
            if !is_header_param_name_allowed(&param.name) {
                return Err(BindError::HeaderNotAllowed {
                    name: param.name.clone(),
                });
            }
            let rendered = scalar_string(&param.name, value)?;
            if rendered.contains(['\r', '\n']) {
                return Err(BindError::HeaderInvalidValue {
                    name: param.name.clone(),
                });
            }
            headers.insert(param.name.clone(), rendered);
        }
        ParamLocation::Body(pointer) => {
            assign_body(body, pointer, value.clone(), &param.name)?;
        }
        // A `Local` param binds into a script's Rhai scope (`script::bindings`) and never
        // reaches an HTTP request — `ApiCall::params` shouldn't declare one, but this is the
        // enforcement point that makes "shouldn't" a hard error rather than a silently dropped
        // value.
        ParamLocation::Local => {
            return Err(BindError::LocalParamNotBindable {
                name: param.name.clone(),
            });
        }
    }
    Ok(())
}

/// A path or header slot holds exactly one string; an array or a nested structure there is a
/// shape mismatch, not something to flatten or stringify further.
fn scalar_string(name: &str, value: &Value) -> Result<String, BindError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Err(shape_error(name, "null")),
        Value::Array(_) => Err(shape_error(name, "array")),
        Value::Object(_) => Err(shape_error(name, "object")),
    }
}

/// A query slot holds one or more strings — a `StringArray` param yields one query pair per
/// element, in array order (I7); any other type yields exactly one.
fn query_strings(name: &str, value: &Value) -> Result<Vec<String>, BindError> {
    match value {
        Value::Array(items) => items.iter().map(|item| scalar_string(name, item)).collect(),
        scalar => scalar_string(name, scalar).map(|s| vec![s]),
    }
}

fn shape_error(name: &str, shape: &'static str) -> BindError {
    BindError::UnsupportedValueShape {
        name: name.to_owned(),
        shape,
    }
}

fn assign_body(
    body: &mut Option<Value>,
    pointer: &PointerBuf,
    value: Value,
    param_name: &str,
) -> Result<(), BindError> {
    let target = body.get_or_insert_with(|| Value::Object(serde_json::Map::new()));
    pointer
        .assign(target, value)
        .map_err(|e| BindError::BodyAssign {
            pointer: pointer.to_string(),
            message: format!("{param_name}: {e}"),
        })?;
    Ok(())
}

/// `form_urlencoded` over a `BTreeMap` iterates keys in sorted order (I7); array elements stay
/// in their original order within each key.
fn encode_query(query_values: &BTreeMap<String, Vec<String>>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, values) in query_values {
        for value in values {
            serializer.append_pair(key, value);
        }
    }
    serializer.finish()
}

/// Builds the full URL as a string from `service`'s scheme/host/port plus the rendered path and
/// query, then re-parses it and asserts nothing moved. Deliberately does **not** use `Url::join`
/// (relative resolution can replace the authority) or `set_path` (it skips `Url::parse`'s
/// dot-segment normalisation, so a smuggled `/a/../b` would go undetected instead of being
/// caught by the comparison below) — this is the containment post-condition I3 depends on.
fn assemble_url(service: &Service, path: &str, query: &str) -> Result<url::Url, BindError> {
    let base = &service.base_url;
    let host_part = match base.host() {
        Some(Host::Domain(d)) => d.to_owned(),
        Some(Host::Ipv4(ip)) => ip.to_string(),
        Some(Host::Ipv6(ip)) => format!("[{ip}]"),
        None => return Err(BindError::NoHost),
    };

    let mut assembled = format!("{}://{host_part}", base.scheme());
    if let Some(port) = base.port() {
        assembled.push_str(&format!(":{port}"));
    }
    assembled.push_str(path);
    if !query.is_empty() {
        assembled.push('?');
        assembled.push_str(query);
    }

    let parsed = url::Url::parse(&assembled).map_err(|e| BindError::UrlParse(e.to_string()))?;

    // `Url::parse` normalises `/a/../b` to `/b`, so comparing the re-parsed path back against
    // the path we intended is what catches that case — the normalisation itself IS the signal.
    let contained = parsed.scheme() == base.scheme()
        && parsed.host_str() == base.host_str()
        && parsed.port_or_known_default() == base.port_or_known_default()
        && parsed.path() == path
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none();

    if !contained {
        return Err(BindError::ContainmentViolation);
    }

    Ok(parsed)
}

#[cfg(test)]
#[path = "bind_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "bind_proptest.rs"]
mod proptests;
