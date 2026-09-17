//! Per-api_call compilation: loads the api_call's service, parses its path template (I3),
//! cross-checks the template's placeholders against its `location = path` params, derives its
//! static origin, and compiles its projection (if any). One api_call's worth of
//! [`super::build_plan`]'s work, split out to keep that function's own line count down.

use std::collections::BTreeSet;

use serde_json_path::JsonPath;
use uuid::Uuid;

use crate::http::{Segment, UrlTemplate};
use crate::model::{ApiCall, ParamLocation, Projection, Slug};
use crate::store::Stores;

use super::ResolveError;
use super::plan::{CompiledProjection, CompiledProjectionField, PlannedApiCall};

pub(super) async fn compile_api_call(
    stores: &Stores,
    owner_id: Uuid,
    api_call: &ApiCall,
) -> Result<PlannedApiCall, ResolveError> {
    let service = stores
        .service()
        .get_by_slug(owner_id, &api_call.service_slug)
        .await
        .map_err(|e| ResolveError::Store(e.to_string()))?
        .ok_or_else(|| {
            ResolveError::Store(format!(
                "api_call {:?} references service {:?}, which no longer exists",
                api_call.slug.as_str(),
                api_call.service_slug.as_str()
            ))
        })?;

    let url_template =
        UrlTemplate::parse(&api_call.path_template).map_err(|source| ResolveError::Template {
            api_call: api_call.slug.as_str().to_owned(),
            source,
        })?;
    assert_path_params_correspond(api_call, &url_template)?;

    // Always `Origin::of(&service.base_url)`: `http::bind::assemble_url` builds every request
    // from the service's scheme/host/port alone, so an api_call's static origin never depends
    // on anything api_call-specific. `base_url` is a `http`/`https` `url::Url` by construction
    // (`store::parse_url`), so an opaque origin here would mean a malformed row, not a
    // reachable branch of normal operation.
    let origin = crate::model::Origin::of(&service.base_url).map_err(|e| {
        ResolveError::Store(format!(
            "service {:?} base_url {:?} has an opaque origin: {e}",
            service.slug.as_str(),
            service.base_url
        ))
    })?;

    let projection = api_call
        .projection
        .as_ref()
        .map(|p| compile_projection(&api_call.slug, p))
        .transpose()?;

    Ok(PlannedApiCall {
        api_call: api_call.clone(),
        service,
        origin,
        url_template,
        projection,
    })
}

/// I3's other half, applied at resolve time: a path template's placeholders must correspond
/// exactly to the api_call's `location = path` params — one placeholder with no matching
/// param can never be rendered (`UrlTemplate::render` would error at call time, on every
/// call); one `path` param with no matching placeholder is a value that can never reach the
/// request it was declared for. Both are definition bugs, and this is the one point that
/// catches them before either surprises a caller at run time.
fn assert_path_params_correspond(
    api_call: &ApiCall,
    template: &UrlTemplate,
) -> Result<(), ResolveError> {
    let placeholders: BTreeSet<String> = template
        .segments()
        .iter()
        .filter_map(|s| match s {
            Segment::Placeholder(name) => Some(name.clone()),
            Segment::Literal(_) => None,
        })
        .collect();
    let path_params: BTreeSet<String> = api_call
        .params
        .iter()
        .filter(|p| p.location == ParamLocation::Path)
        .map(|p| p.name.clone())
        .collect();

    if placeholders != path_params {
        return Err(ResolveError::PathParamMismatch {
            api_call: api_call.slug.as_str().to_owned(),
            placeholders,
            path_params,
        });
    }
    Ok(())
}

fn compile_projection(
    api_call: &Slug,
    projection: &Projection,
) -> Result<CompiledProjection, ResolveError> {
    let fields = projection
        .fields
        .iter()
        .map(|f| {
            let path = JsonPath::parse(&f.path).map_err(|e| ResolveError::Projection {
                api_call: api_call.as_str().to_owned(),
                field: f.name.clone(),
                message: e.to_string(),
            })?;
            Ok(CompiledProjectionField {
                name: f.name.clone(),
                path,
                cardinality: f.cardinality,
                coerce: f.coerce,
            })
        })
        .collect::<Result<Vec<_>, ResolveError>>()?;
    Ok(CompiledProjection { fields })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::model::{Access, Pagination};

    fn sample_api_call() -> ApiCall {
        ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: "call-a".parse().unwrap(),
            service_slug: "svc".parse().unwrap(),
            auth_provider_slug: None,
            method: http::Method::GET,
            path_template: "/users/{id}".to_owned(),
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

    #[test]
    fn path_params_must_correspond_exactly() {
        let template = UrlTemplate::parse("/users/{id}").unwrap();
        let mut api_call = sample_api_call();
        // No `path`-location param at all: placeholder `id` is orphaned.
        let err = assert_path_params_correspond(&api_call, &template).unwrap_err();
        assert!(matches!(err, ResolveError::PathParamMismatch { .. }));

        api_call.params.push(crate::model::Param {
            name: "id".to_owned(),
            location: ParamLocation::Path,
            ty: crate::model::ParamType::String,
            required: true,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        });
        assert!(assert_path_params_correspond(&api_call, &template).is_ok());
    }

    #[test]
    fn extra_path_param_with_no_placeholder_is_also_a_mismatch() {
        let template = UrlTemplate::parse("/things").unwrap();
        let mut api_call = sample_api_call();
        api_call.path_template = "/things".to_owned();
        api_call.params.push(crate::model::Param {
            name: "id".to_owned(),
            location: ParamLocation::Path,
            ty: crate::model::ParamType::String,
            required: true,
            default: None,
            fixed: None,
            enum_values: None,
            description: None,
            position: 0,
        });
        let err = assert_path_params_correspond(&api_call, &template).unwrap_err();
        assert!(matches!(err, ResolveError::PathParamMismatch { .. }));
    }
}
