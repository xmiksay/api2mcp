//! api_call/script/endpoint checks — split out of [`super`] purely to keep that file under the
//! workspace's 400-line cap. Every `pub(super)` function here pushes onto the caller's
//! `Vec<ValidationError>` rather than returning early, so [`super::validate`] keeps collecting
//! every failure across every item.

use std::collections::BTreeSet;

use serde_json_path::JsonPath;

use crate::http::UrlTemplate;
use crate::model::is_header_param_name_allowed;

use super::ValidationError;
use crate::pack::{
    Pack, PackApiCall, PackEndpoint, PackEndpointTarget, PackPagination, PackParamLocation,
    PackScript,
};

pub(super) fn validate_api_call(
    errors: &mut Vec<ValidationError>,
    pack: &Pack,
    slug: &str,
    call: &PackApiCall,
) {
    let push = |errors: &mut Vec<ValidationError>, message: String| {
        errors.push(ValidationError::ApiCall {
            slug: slug.to_owned(),
            message,
        });
    };

    if !pack.services.contains_key(&call.service) {
        push(
            errors,
            format!(
                "references service {:?}, which is not in this pack",
                call.service
            ),
        );
    }
    if let Some(provider_slug) = &call.auth_provider {
        match pack.auth_providers.get(provider_slug) {
            None => push(
                errors,
                format!("references auth_provider {provider_slug:?}, which is not in this pack"),
            ),
            Some(p) if p.service != call.service => push(
                errors,
                format!(
                    "auth_provider {provider_slug:?} belongs to service {:?}, not this api_call's {:?}",
                    p.service, call.service
                ),
            ),
            Some(_) => {}
        }
    }
    if http::Method::from_bytes(call.method.as_bytes()).is_err() {
        push(
            errors,
            format!("method {:?} is not a valid HTTP method", call.method),
        );
    }
    if call.access != "read" && call.access != "write" {
        push(
            errors,
            format!("access {:?}: expected \"read\" or \"write\"", call.access),
        );
    }

    match UrlTemplate::parse(&call.path_template) {
        Ok(template) => validate_path_params(errors, slug, &template, call),
        Err(e) => push(
            errors,
            format!("path_template {:?}: {e}", call.path_template),
        ),
    }

    if let Some(projection) = &call.projection {
        for field in &projection.fields {
            if let Err(e) = JsonPath::parse(&field.path) {
                push(
                    errors,
                    format!(
                        "projection field {:?} path {:?}: {e}",
                        field.name, field.path
                    ),
                );
            }
        }
    }

    if let PackPagination::Cursor {
        next_cursor_path, ..
    } = &call.pagination
        && next_cursor_path.parse::<jsonptr::PointerBuf>().is_err()
    {
        push(
            errors,
            format!("pagination next_cursor_path {next_cursor_path:?} is not a valid JSON pointer"),
        );
    }

    for p in &call.params {
        if p.fixed.is_some() && p.required {
            push(
                errors,
                format!("param {:?} is both fixed and required", p.name),
            );
        }
        // Publish-time half of `model::HEADER_PARAM_ALLOWLIST` (I3): everything else in this
        // system validates at publish time precisely so a definition cannot be saved broken —
        // `http::bind`'s own check at dispatch time stays as defence in depth, not the only gate.
        if p.location == PackParamLocation::Header && !is_header_param_name_allowed(&p.name) {
            push(
                errors,
                format!(
                    "param {:?} uses location: header with a name not in the allowlist ({:?}); \
                     allowed names are {:?}",
                    p.name,
                    p.name,
                    crate::model::HEADER_PARAM_ALLOWLIST
                ),
            );
        }
    }
}

/// I3's other half, checked here too (not just at resolve time): a path template's placeholders
/// must correspond exactly to the api_call's `location = path` params.
fn validate_path_params(
    errors: &mut Vec<ValidationError>,
    slug: &str,
    template: &UrlTemplate,
    call: &PackApiCall,
) {
    let placeholders: BTreeSet<&str> = template
        .segments()
        .iter()
        .filter_map(|s| match s {
            crate::http::Segment::Placeholder(name) => Some(name.as_str()),
            crate::http::Segment::Literal(_) => None,
        })
        .collect();
    let path_params: BTreeSet<&str> = call
        .params
        .iter()
        .filter(|p| p.location == crate::pack::PackParamLocation::Path)
        .map(|p| p.name.as_str())
        .collect();
    if placeholders != path_params {
        errors.push(ValidationError::ApiCall {
            slug: slug.to_owned(),
            message: format!(
                "path template placeholders {placeholders:?} do not match its `location: path` params {path_params:?}"
            ),
        });
    }
}

pub(super) fn validate_script(
    errors: &mut Vec<ValidationError>,
    pack: &Pack,
    slug: &str,
    script: &PackScript,
) {
    for (alias, target) in &script.callable {
        if !pack.api_calls.contains_key(target) {
            errors.push(ValidationError::Script {
                slug: slug.to_owned(),
                message: format!(
                    "callable[{alias:?}] names api_call {target:?}, which is not in this pack"
                ),
            });
        }
    }
    for p in &script.params {
        if p.location != crate::pack::PackParamLocation::Local {
            errors.push(ValidationError::Script {
                slug: slug.to_owned(),
                message: format!("param {:?} must use location: local", p.name),
            });
        }
    }
}

pub(super) fn validate_endpoint(
    errors: &mut Vec<ValidationError>,
    pack: &Pack,
    slug: &str,
    endpoint: &PackEndpoint,
) {
    if let Err(e) = crate::resolve::tag_expr::parse(&endpoint.tag_expr) {
        // `TagExprError`'s own `Display` already reads "endpoints.tag_expr {input:?}: ...", so
        // this doesn't re-wrap it with a second "tag_expr {:?}:" prefix.
        errors.push(ValidationError::Endpoint {
            slug: slug.to_owned(),
            message: e.to_string(),
        });
    }
    if endpoint.write_ceiling != "read" && endpoint.write_ceiling != "write" {
        errors.push(ValidationError::Endpoint {
            slug: slug.to_owned(),
            message: format!(
                "write_ceiling {:?}: expected \"read\" or \"write\"",
                endpoint.write_ceiling
            ),
        });
    }
    for target in endpoint.aliases.values() {
        let missing = match target {
            PackEndpointTarget::ApiCall(s) => !pack.api_calls.contains_key(s),
            PackEndpointTarget::Script(s) => !pack.scripts.contains_key(s),
        };
        if missing {
            errors.push(ValidationError::Endpoint {
                slug: slug.to_owned(),
                message: format!("alias target {target:?} is not in this pack"),
            });
        }
    }
    for provider in &endpoint.auth_providers {
        if !pack.auth_providers.contains_key(provider) {
            errors.push(ValidationError::Endpoint {
                slug: slug.to_owned(),
                message: format!(
                    "auth_providers scope names {provider:?}, which is not in this pack"
                ),
            });
        }
    }
}

#[cfg(test)]
#[path = "items_tests.rs"]
mod tests;
