//! `model::` <-> `pack::Pack*` conversions for the admin API's DTOs (see `dto`'s module doc for
//! why a DTO body *is* a `Pack*` shape). This mirrors `pack::convert`'s own logic rather than
//! calling it: that module is private to `pack::` (see its own doc — "the small duplication ...
//! is the cost of that boundary, not an oversight" — for the identical situation `pack::convert`
//! itself is in relative to `store::api_call_params`), so `server::api` cannot name it regardless
//! of any individual function's own visibility, and `src/pack/` is out of scope for this chunk to
//! change. Every error here is a plain `String` (not a typed enum): these are HTTP input-shape
//! problems reported as a single [`crate::server::error::ApiError::BadRequest`], not part of the
//! "report every failure at once" contract — that contract belongs to `pack::validate`, run
//! separately by `validate_write` once conversion has already produced a well-typed value.
//!
//! Split into this file (scalars: `Param`/`Projection`/`Pagination`/`Budgets`/`Service`/
//! `AuthProvider`/tags) and [`super::convert_items`] (`ApiCall`/`ScriptDef`/`EndpointDef`, which
//! compose the former) to keep both under the workspace's 400-line cap.

use std::collections::BTreeSet;

use crate::model::{
    Access, AuthKind, AuthProvider, Budgets, Cardinality, Origin, Pagination, Param, ParamLocation,
    ParamType, Projection, ProjectionField, Service, Slug, Tag,
};
use crate::pack::{
    PackAuthKind, PackAuthProvider, PackBudgets, PackCardinality, PackPagination, PackParam,
    PackParamLocation, PackProjection, PackProjectionField, PackService,
};

pub fn parse_slug(s: &str) -> Result<Slug, String> {
    s.parse().map_err(|e| format!("slug {s:?}: {e}"))
}

pub fn parse_url(s: &str) -> Result<url::Url, String> {
    url::Url::parse(s).map_err(|e| format!("url {s:?}: {e}"))
}

pub fn parse_origin(s: &str) -> Result<Origin, String> {
    s.parse()
        .map_err(|e: crate::model::OriginError| format!("origin {s:?}: {e}"))
}

pub fn access_to_str(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "write",
    }
}

pub fn access_from_str(s: &str) -> Result<Access, String> {
    match s {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(format!("access {other:?}: expected \"read\" or \"write\"")),
    }
}

pub fn param_type_to_str(t: ParamType) -> &'static str {
    match t {
        ParamType::String => "string",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
        ParamType::StringArray => "string_array",
    }
}

pub fn param_type_from_str(s: &str) -> Result<ParamType, String> {
    Ok(match s {
        "string" => ParamType::String,
        "integer" => ParamType::Integer,
        "number" => ParamType::Number,
        "boolean" => ParamType::Boolean,
        "string_array" => ParamType::StringArray,
        other => {
            return Err(format!(
                "param type {other:?}: expected string|integer|number|boolean|string_array"
            ));
        }
    })
}

fn parse_pointer(raw: &str) -> Result<jsonptr::PointerBuf, String> {
    raw.parse::<jsonptr::PointerBuf>()
        .map_err(|e| format!("json pointer {raw:?}: {e}"))
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

fn param_location_from_pack(l: &PackParamLocation) -> Result<ParamLocation, String> {
    Ok(match l {
        PackParamLocation::Path => ParamLocation::Path,
        PackParamLocation::Query => ParamLocation::Query,
        PackParamLocation::Header => ParamLocation::Header,
        PackParamLocation::Body(ptr) => ParamLocation::Body(parse_pointer(ptr)?),
        PackParamLocation::Local => ParamLocation::Local,
    })
}

pub fn param_to_pack(p: &Param) -> PackParam {
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

pub fn param_from_pack(p: &PackParam) -> Result<Param, String> {
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

pub fn projection_to_pack(p: &Projection) -> PackProjection {
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

pub fn projection_from_pack(p: &PackProjection) -> Result<Projection, String> {
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
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Projection { fields })
}

pub fn pagination_to_pack(p: &Pagination) -> PackPagination {
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

pub fn pagination_from_pack(p: &PackPagination) -> Result<Pagination, String> {
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

pub fn budgets_to_pack(b: &Budgets) -> PackBudgets {
    PackBudgets {
        max_calls: b.max_calls,
        max_bytes: b.max_bytes,
        wall_clock_ms: b.wall_clock.map(|d| d.as_millis() as u64),
        max_pages: b.max_pages,
        max_concurrency: b.max_concurrency,
    }
}

pub fn budgets_from_pack(b: &PackBudgets) -> Budgets {
    Budgets {
        max_calls: b.max_calls,
        max_bytes: b.max_bytes,
        wall_clock: b.wall_clock_ms.map(std::time::Duration::from_millis),
        max_pages: b.max_pages,
        max_concurrency: b.max_concurrency,
    }
}

pub fn service_to_pack(s: &Service) -> PackService {
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

pub fn service_from_pack(slug: Slug, s: &PackService) -> Result<Service, String> {
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

pub fn auth_provider_to_pack(p: &AuthProvider) -> PackAuthProvider {
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

pub fn auth_provider_from_pack(
    slug: Slug,
    service_slug: Slug,
    p: &PackAuthProvider,
) -> Result<AuthProvider, String> {
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

pub fn tags_to_pack(tags: &BTreeSet<Tag>) -> BTreeSet<String> {
    tags.iter().map(|t| t.0.as_str().to_owned()).collect()
}

pub fn tags_from_pack(tags: &BTreeSet<String>) -> Result<BTreeSet<Tag>, String> {
    tags.iter()
        .map(|t| Ok(Tag(parse_slug(t)?)))
        .collect::<Result<BTreeSet<_>, String>>()
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
    fn service_round_trips_through_pack_shape() {
        let base_url: url::Url = "https://svc.example.com/".parse().unwrap();
        let service = Service {
            slug: "svc".parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: std::collections::BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: Some(10),
            max_response_bytes: 1_000_000,
        };
        let pack = service_to_pack(&service);
        let back = service_from_pack(service.slug.clone(), &pack).unwrap();
        assert_eq!(back, service);
    }

    #[test]
    fn tags_round_trip() {
        let tags = BTreeSet::from([Tag("read".parse().unwrap()), Tag("write".parse().unwrap())]);
        let pack = tags_to_pack(&tags);
        assert_eq!(tags_from_pack(&pack).unwrap(), tags);
    }

    #[test]
    fn malformed_pointer_is_rejected() {
        let err = param_location_from_pack(&PackParamLocation::Body("no-leading-slash".into()))
            .unwrap_err();
        assert!(err.contains("json pointer"));
    }
}
