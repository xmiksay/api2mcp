//! `ApiCall`/`ScriptDef`/`EndpointDef` conversions — the shapes that compose the scalars in
//! [`super`], split out purely to keep that file under the workspace's 400-line cap.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use uuid::Uuid;

use crate::model::{ApiCall, EndpointDef, EndpointTarget, ScriptDef, Slug, Tag};
use crate::resolve::tag_expr;

use super::{
    ConvertError, access_from_str, access_to_str, budgets_from_pack, budgets_to_pack,
    pagination_from_pack, pagination_to_pack, param_from_pack, param_to_pack, parse_slug,
    projection_from_pack, projection_to_pack,
};
use crate::pack::{PackApiCall, PackEndpoint, PackEndpointTarget, PackScript};

pub(crate) fn api_call_to_pack(c: &ApiCall, tags: &BTreeSet<Tag>) -> PackApiCall {
    PackApiCall {
        service: c.service_slug.as_str().to_owned(),
        method: c.method.as_str().to_owned(),
        path_template: c.path_template.clone(),
        query_fixed: c.query_fixed.clone(),
        body_template: c.body_template.clone(),
        access: access_to_str(c.access).to_owned(),
        idempotent: c.idempotent,
        projection: c.projection.as_ref().map(projection_to_pack),
        pagination: pagination_to_pack(&c.pagination),
        timeout_ms: c.timeout_ms,
        max_response_bytes: c.max_response_bytes,
        params: c.params.iter().map(param_to_pack).collect(),
        tags: tags.iter().map(|t| t.0.as_str().to_owned()).collect(),
        description: c.description.clone(),
    }
}

pub(crate) fn api_call_from_pack(
    owner_id: Uuid,
    slug: Slug,
    service_slug: Slug,
    c: &PackApiCall,
) -> Result<ApiCall, ConvertError> {
    let method = http::Method::from_str(&c.method)
        .map_err(|e| ConvertError::Method(c.method.clone(), e.to_string()))?;
    Ok(ApiCall {
        owner_id,
        slug,
        service_slug,
        method,
        path_template: c.path_template.clone(),
        query_fixed: c.query_fixed.clone(),
        body_template: c.body_template.clone(),
        access: access_from_str(&c.access)?,
        idempotent: c.idempotent,
        projection: c
            .projection
            .as_ref()
            .map(projection_from_pack)
            .transpose()?,
        pagination: pagination_from_pack(&c.pagination)?,
        timeout_ms: c.timeout_ms,
        max_response_bytes: c.max_response_bytes,
        params: c
            .params
            .iter()
            .map(param_from_pack)
            .collect::<Result<Vec<_>, _>>()?,
        description: c.description.clone(),
    })
}

pub(crate) fn script_to_pack(s: &ScriptDef, tags: &BTreeSet<Tag>) -> PackScript {
    PackScript {
        source: s.source.clone(),
        params: s.params.iter().map(param_to_pack).collect(),
        callable: s
            .callable
            .iter()
            .map(|(alias, target)| (alias.clone(), target.as_str().to_owned()))
            .collect(),
        budgets: budgets_to_pack(&s.budgets),
        description: s.description.clone(),
        tags: tags.iter().map(|t| t.0.as_str().to_owned()).collect(),
    }
}

pub(crate) fn script_from_pack(
    owner_id: Uuid,
    slug: Slug,
    s: &PackScript,
) -> Result<ScriptDef, ConvertError> {
    let callable = s
        .callable
        .iter()
        .map(|(alias, target)| Ok((alias.clone(), parse_slug(target)?)))
        .collect::<Result<BTreeMap<_, _>, ConvertError>>()?;
    Ok(ScriptDef {
        owner_id,
        slug,
        source: s.source.clone(),
        params: s
            .params
            .iter()
            .map(param_from_pack)
            .collect::<Result<Vec<_>, _>>()?,
        callable,
        budgets: budgets_from_pack(&s.budgets),
        description: s.description.clone(),
    })
}

fn endpoint_target_to_pack(t: &EndpointTarget) -> PackEndpointTarget {
    match t {
        EndpointTarget::ApiCall(s) => PackEndpointTarget::ApiCall(s.as_str().to_owned()),
        EndpointTarget::Script(s) => PackEndpointTarget::Script(s.as_str().to_owned()),
    }
}

fn endpoint_target_from_pack(t: &PackEndpointTarget) -> Result<EndpointTarget, ConvertError> {
    Ok(match t {
        PackEndpointTarget::ApiCall(s) => EndpointTarget::ApiCall(parse_slug(s)?),
        PackEndpointTarget::Script(s) => EndpointTarget::Script(parse_slug(s)?),
    })
}

pub(crate) fn endpoint_to_pack(e: &EndpointDef) -> PackEndpoint {
    PackEndpoint {
        tag_expr: tag_expr::to_string(&e.tag_expr),
        write_ceiling: access_to_str(e.write_ceiling).to_owned(),
        budgets: budgets_to_pack(&e.budgets),
        instructions: e.instructions.clone(),
        enabled: e.enabled,
        aliases: e
            .aliases
            .iter()
            .map(|(a, t)| (a.clone(), endpoint_target_to_pack(t)))
            .collect(),
        auth_providers: e
            .auth_providers
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect(),
    }
}

pub(crate) fn endpoint_from_pack(
    owner_id: Uuid,
    slug: Slug,
    e: &PackEndpoint,
) -> Result<EndpointDef, ConvertError> {
    let tag_expr = tag_expr::parse(&e.tag_expr)
        .map_err(|err| ConvertError::TagExprParse(e.tag_expr.clone(), err.to_string()))?;
    let aliases = e
        .aliases
        .iter()
        .map(|(a, t)| Ok((a.clone(), endpoint_target_from_pack(t)?)))
        .collect::<Result<BTreeMap<_, _>, ConvertError>>()?;
    let auth_providers = e
        .auth_providers
        .iter()
        .map(|s| parse_slug(s))
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(EndpointDef {
        owner_id,
        slug,
        tag_expr,
        write_ceiling: access_from_str(&e.write_ceiling)?,
        budgets: budgets_from_pack(&e.budgets),
        instructions: e.instructions.clone(),
        enabled: e.enabled,
        aliases,
        auth_providers,
    })
}
