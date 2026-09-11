//! Conversions between the wire ([`super::Pack*`]) shapes and [`crate::model`] types. Split into
//! this file (scalars, `Param`/`Projection`/`Pagination`/`Budgets`/`Service`/`AuthProvider`) and
//! [`items`] (`ApiCall`/`ScriptDef`/`EndpointDef`, which compose the former) to keep both under
//! the workspace's 400-line cap.
//!
//! Deliberately not `crate::store`'s `parse_slug`/`parse_url`/`parse_origin`/`access_to_str`/
//! `str_to_access` even though those already exist: they return `StoreError`, and threading that
//! through here would make [`ConvertError`] (which must stay usable from [`super::validate`], a
//! DB-free module) depend on a type whose own doc says it exists to keep raw `DbErr` detail away
//! from anything DB-adjacent. `data_type_to_str`/`str_to_data_type` and the projection/pagination
//! JSON mirrors in `store::api_call_params`/`store::api_call_projection` aren't even reachable
//! from here — both are private submodules of `store`, so only `store::` itself (whose own items
//! are declared directly in `store/mod.rs`) is nameable from outside it. The small duplication
//! below is the cost of that boundary, not an oversight.

mod items;

pub(crate) use items::{
    api_call_from_pack, api_call_to_pack, endpoint_from_pack, endpoint_to_pack, script_from_pack,
    script_to_pack,
};

use std::collections::BTreeSet;

use crate::model::{
    Access, AuthKind, AuthProvider, Budgets, Cardinality, Origin, Pagination, Param, ParamLocation,
    ParamType, Projection, ProjectionField, Service, Slug, SlugError, Tag,
};

use super::{
    PackAuthKind, PackAuthProvider, PackBudgets, PackCardinality, PackPagination, PackParam,
    PackParamLocation, PackProjection, PackProjectionField, PackService,
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConvertError {
    #[error("slug {0:?}: {1}")]
    Slug(String, SlugError),
    #[error("url {0:?}: {1}")]
    Url(String, String),
    #[error("origin {0:?}: {1}")]
    Origin(String, String),
    #[error("http method {0:?}: {1}")]
    Method(String, String),
    #[error("access {0:?}: expected \"read\" or \"write\"")]
    Access(String),
    #[error("param type {0:?}: expected string|integer|number|boolean|string_array")]
    ParamType(String),
    #[error("cardinality {0:?}: expected \"one\" or \"many\"")]
    Cardinality(String),
    #[error("json pointer {0:?}: {1}")]
    Pointer(String, String),
    #[error("tag_expr {0:?}: {1}")]
    TagExprParse(String, String),
}

pub(crate) fn parse_slug(s: &str) -> Result<Slug, ConvertError> {
    s.parse().map_err(|e| ConvertError::Slug(s.to_owned(), e))
}

pub(crate) fn parse_url(s: &str) -> Result<url::Url, ConvertError> {
    url::Url::parse(s).map_err(|e| ConvertError::Url(s.to_owned(), e.to_string()))
}

pub(crate) fn parse_origin(s: &str) -> Result<Origin, ConvertError> {
    s.parse()
        .map_err(|e: crate::model::OriginError| ConvertError::Origin(s.to_owned(), e.to_string()))
}

pub(crate) fn access_to_str(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "write",
    }
}

pub(crate) fn access_from_str(s: &str) -> Result<Access, ConvertError> {
    match s {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(ConvertError::Access(other.to_owned())),
    }
}

pub(crate) fn param_type_to_str(t: ParamType) -> &'static str {
    match t {
        ParamType::String => "string",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
        ParamType::StringArray => "string_array",
    }
}

pub(crate) fn param_type_from_str(s: &str) -> Result<ParamType, ConvertError> {
    Ok(match s {
        "string" => ParamType::String,
        "integer" => ParamType::Integer,
        "number" => ParamType::Number,
        "boolean" => ParamType::Boolean,
        "string_array" => ParamType::StringArray,
        other => return Err(ConvertError::ParamType(other.to_owned())),
    })
}

pub(crate) fn parse_pointer(raw: &str) -> Result<jsonptr::PointerBuf, ConvertError> {
    raw.parse::<jsonptr::PointerBuf>()
        .map_err(|e| ConvertError::Pointer(raw.to_owned(), e.to_string()))
}

fn param_location_to_pack(l: &ParamLocation) -> PackParamLocation {
    match l {
        ParamLocation::Path => PackParamLocation::Path,
        ParamLocation::Query => PackParamLocation::Query,
        ParamLocation::Header => PackParamLocation::Header,
        ParamLocation::Body(ptr) => PackParamLocation::Body(ptr.to_string()),
        ParamLocation::Local => PackParamLocation::Local,
    }
}

fn param_location_from_pack(l: &PackParamLocation) -> Result<ParamLocation, ConvertError> {
    Ok(match l {
        PackParamLocation::Path => ParamLocation::Path,
        PackParamLocation::Query => ParamLocation::Query,
        PackParamLocation::Header => ParamLocation::Header,
        PackParamLocation::Body(ptr) => ParamLocation::Body(parse_pointer(ptr)?),
        PackParamLocation::Local => ParamLocation::Local,
    })
}

pub(crate) fn param_to_pack(p: &Param) -> PackParam {
    PackParam {
        name: p.name.clone(),
        location: param_location_to_pack(&p.location),
        ty: param_type_to_str(p.ty).to_owned(),
        required: p.required,
        default: p.default.clone(),
        fixed: p.fixed.clone(),
        enum_values: p.enum_values.clone(),
        description: p.description.clone(),
        position: p.position,
    }
}

pub(crate) fn param_from_pack(p: &PackParam) -> Result<Param, ConvertError> {
    Ok(Param {
        name: p.name.clone(),
        location: param_location_from_pack(&p.location)?,
        ty: param_type_from_str(&p.ty)?,
        required: p.required,
        default: p.default.clone(),
        fixed: p.fixed.clone(),
        enum_values: p.enum_values.clone(),
        description: p.description.clone(),
        position: p.position,
    })
}

pub(crate) fn projection_to_pack(p: &Projection) -> PackProjection {
    PackProjection {
        fields: p
            .fields
            .iter()
            .map(|f| PackProjectionField {
                name: f.name.clone(),
                path: f.path.clone(),
                cardinality: match f.cardinality {
                    Cardinality::One => PackCardinality::One,
                    Cardinality::Many => PackCardinality::Many,
                },
                coerce: f.coerce.map(param_type_to_str).map(str::to_owned),
            })
            .collect(),
    }
}

pub(crate) fn projection_from_pack(p: &PackProjection) -> Result<Projection, ConvertError> {
    let fields = p
        .fields
        .iter()
        .map(|f| {
            Ok(ProjectionField {
                name: f.name.clone(),
                path: f.path.clone(),
                cardinality: match f.cardinality {
                    PackCardinality::One => Cardinality::One,
                    PackCardinality::Many => Cardinality::Many,
                },
                coerce: f.coerce.as_deref().map(param_type_from_str).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, ConvertError>>()?;
    Ok(Projection { fields })
}

pub(crate) fn pagination_to_pack(p: &Pagination) -> PackPagination {
    match p {
        Pagination::None => PackPagination::None,
        Pagination::Cursor {
            next_cursor_path,
            query_param,
        } => PackPagination::Cursor {
            next_cursor_path: next_cursor_path.to_string(),
            query_param: query_param.clone(),
        },
    }
}

pub(crate) fn pagination_from_pack(p: &PackPagination) -> Result<Pagination, ConvertError> {
    Ok(match p {
        PackPagination::None => Pagination::None,
        PackPagination::Cursor {
            next_cursor_path,
            query_param,
        } => Pagination::Cursor {
            next_cursor_path: parse_pointer(next_cursor_path)?,
            query_param: query_param.clone(),
        },
    })
}

pub(crate) fn budgets_to_pack(b: &Budgets) -> PackBudgets {
    PackBudgets {
        max_calls: b.max_calls,
        max_bytes: b.max_bytes,
        wall_clock_ms: b.wall_clock.map(|d| d.as_millis() as u64),
        max_pages: b.max_pages,
        max_concurrency: b.max_concurrency,
    }
}

pub(crate) fn budgets_from_pack(b: &PackBudgets) -> Budgets {
    Budgets {
        max_calls: b.max_calls,
        max_bytes: b.max_bytes,
        wall_clock: b.wall_clock_ms.map(std::time::Duration::from_millis),
        max_pages: b.max_pages,
        max_concurrency: b.max_concurrency,
    }
}

pub(crate) fn service_to_pack(s: &Service) -> PackService {
    PackService {
        base_url: s.base_url.to_string(),
        origin_allowlist: s.origin_allowlist.iter().map(|o| o.to_string()).collect(),
        default_headers: s.default_headers.clone(),
        timeout_ms: s.timeout_ms,
        max_concurrency: s.max_concurrency,
        rate_limit_per_min: s.rate_limit_per_min,
        max_response_bytes: s.max_response_bytes,
    }
}

pub(crate) fn service_from_pack(slug: Slug, s: &PackService) -> Result<Service, ConvertError> {
    let base_url = parse_url(&s.base_url)?;
    let origin_allowlist = s
        .origin_allowlist
        .iter()
        .map(|o| parse_origin(o))
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(Service {
        slug,
        base_url,
        origin_allowlist,
        default_headers: s.default_headers.clone(),
        timeout_ms: s.timeout_ms,
        max_concurrency: s.max_concurrency,
        rate_limit_per_min: s.rate_limit_per_min,
        max_response_bytes: s.max_response_bytes,
    })
}

fn auth_kind_to_pack(k: &AuthKind) -> PackAuthKind {
    match k {
        AuthKind::StaticHeader => PackAuthKind::StaticHeader,
        AuthKind::OAuth2ClientCredentials => PackAuthKind::OAuth2ClientCredentials,
    }
}

fn auth_kind_from_pack(k: PackAuthKind) -> AuthKind {
    match k {
        PackAuthKind::StaticHeader => AuthKind::StaticHeader,
        PackAuthKind::OAuth2ClientCredentials => AuthKind::OAuth2ClientCredentials,
    }
}

pub(crate) fn auth_provider_to_pack(p: &AuthProvider) -> PackAuthProvider {
    PackAuthProvider {
        service: p.service_slug.as_str().to_owned(),
        kind: auth_kind_to_pack(&p.kind),
        credential_env_key: p.credential_env_key.clone(),
        header_name: p.header_name.clone(),
        value_template: p.value_template.clone(),
        scopes: p.scopes.clone(),
        token_url: p.token_url.as_ref().map(|u| u.to_string()),
        bound_origin: p.bound_origin.to_string(),
    }
}

pub(crate) fn auth_provider_from_pack(
    slug: Slug,
    service_slug: Slug,
    p: &PackAuthProvider,
) -> Result<AuthProvider, ConvertError> {
    Ok(AuthProvider {
        slug,
        service_slug,
        kind: auth_kind_from_pack(p.kind),
        credential_env_key: p.credential_env_key.clone(),
        header_name: p.header_name.clone(),
        value_template: p.value_template.clone(),
        scopes: p.scopes.clone(),
        token_url: p.token_url.as_deref().map(parse_url).transpose()?,
        bound_origin: parse_origin(&p.bound_origin)?,
    })
}

pub(crate) fn tags_from_pack(tags: &BTreeSet<String>) -> Result<BTreeSet<Tag>, ConvertError> {
    tags.iter()
        .map(|t| Ok(Tag(parse_slug(t)?)))
        .collect::<Result<BTreeSet<_>, ConvertError>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_round_trips() {
        assert_eq!(
            access_from_str(access_to_str(Access::Read)).unwrap(),
            Access::Read
        );
        assert_eq!(
            access_from_str(access_to_str(Access::Write)).unwrap(),
            Access::Write
        );
        assert!(access_from_str("nonsense").is_err());
    }

    #[test]
    fn param_type_round_trips_every_variant() {
        for ty in [
            ParamType::String,
            ParamType::Integer,
            ParamType::Number,
            ParamType::Boolean,
            ParamType::StringArray,
        ] {
            assert_eq!(param_type_from_str(param_type_to_str(ty)).unwrap(), ty);
        }
    }

    #[test]
    fn body_pointer_round_trips_through_pack_param_location() {
        let ptr = jsonptr::PointerBuf::from_tokens(["user", "email"]);
        let pack = param_location_to_pack(&ParamLocation::Body(ptr.clone()));
        assert_eq!(pack, PackParamLocation::Body("/user/email".to_owned()));
        let back = param_location_from_pack(&pack).unwrap();
        assert_eq!(back, ParamLocation::Body(ptr));
    }

    #[test]
    fn malformed_pointer_is_rejected() {
        let err = param_location_from_pack(&PackParamLocation::Body("no-leading-slash".into()))
            .unwrap_err();
        assert!(matches!(err, ConvertError::Pointer(_, _)));
    }
}
