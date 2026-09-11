//! The pre-import gate, run **before any transaction opens** — this module never touches a
//! database. Every check here is over the [`Pack`] value alone: slugs unique and well-formed,
//! URL templates compile, projections compile, tag expressions parse, every api_call a script
//! declares exists in the pack, and every auth provider's `bound_origin` is inside its service's
//! origin allowlist. [`validate`] reports **every** failure it finds, not just the first —
//! someone fixing a hand-written pack should not have to fix one error, re-run, and repeat.
//!
//! Split into this file (version, slugs, services, auth providers, the tag vocabulary) and
//! [`items`] (api_calls, scripts, endpoints — the shapes that reference the former) to keep both
//! under the workspace's 400-line cap.

mod items;

use std::collections::BTreeSet;

use super::convert;
use super::validate_credentials;
use super::{Pack, PackAuthProvider, PackService};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("unsupported pack version {found} (this build writes and accepts only {expected})")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("{location}: {message}")]
    Slug { location: String, message: String },
    #[error("services.{slug}: {message}")]
    Service { slug: String, message: String },
    #[error("auth_providers.{slug}: {message}")]
    AuthProvider { slug: String, message: String },
    #[error("api_calls.{slug}: {message}")]
    ApiCall { slug: String, message: String },
    #[error("scripts.{slug}: {message}")]
    Script { slug: String, message: String },
    #[error("endpoints.{slug}: {message}")]
    Endpoint { slug: String, message: String },
    #[error("tags: {0}")]
    Tags(String),
    #[error("{location}.{field} looks like it contains a credential value, not a reference to one")]
    Credential { location: String, field: String },
}

/// Validates `pack` in isolation, collecting every failure it can find rather than stopping at
/// the first. `Ok(())` means `pack` is fit to hand to [`super::import::import`].
pub fn validate(pack: &Pack) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if pack.version != super::PACK_VERSION {
        errors.push(ValidationError::UnsupportedVersion {
            found: pack.version,
            expected: super::PACK_VERSION,
        });
    }

    for (slug, svc) in &pack.services {
        check_slug(&mut errors, "services", slug);
        validate_service(&mut errors, slug, svc);
    }
    for (slug, provider) in &pack.auth_providers {
        check_slug(&mut errors, "auth_providers", slug);
        validate_auth_provider(&mut errors, pack, slug, provider);
    }
    for (slug, call) in &pack.api_calls {
        check_slug(&mut errors, "api_calls", slug);
        items::validate_api_call(&mut errors, pack, slug, call);
    }
    for (slug, script) in &pack.scripts {
        check_slug(&mut errors, "scripts", slug);
        items::validate_script(&mut errors, pack, slug, script);
    }
    for (slug, endpoint) in &pack.endpoints {
        check_slug(&mut errors, "endpoints", slug);
        items::validate_endpoint(&mut errors, pack, slug, endpoint);
    }

    validate_tag_vocabulary(&mut errors, pack);
    validate_credentials::scan(pack, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_slug(errors: &mut Vec<ValidationError>, category: &'static str, slug: &str) {
    if let Err(e) = convert::parse_slug(slug) {
        errors.push(ValidationError::Slug {
            location: format!("{category}.{slug}"),
            message: e.to_string(),
        });
    }
}

fn validate_service(errors: &mut Vec<ValidationError>, slug: &str, svc: &PackService) {
    let base_url = match url::Url::parse(&svc.base_url) {
        Ok(u) => Some(u),
        Err(e) => {
            errors.push(ValidationError::Service {
                slug: slug.to_owned(),
                message: format!("base_url {:?}: {e}", svc.base_url),
            });
            None
        }
    };
    for o in &svc.origin_allowlist {
        if o.parse::<crate::model::Origin>().is_err() {
            errors.push(ValidationError::Service {
                slug: slug.to_owned(),
                message: format!("origin_allowlist entry {o:?} is not a valid origin"),
            });
        }
    }
    // Design correction #7: a service's own origin must be a member of its allowlist, or the
    // allowlist is trivially inconsistent with what it's supposed to bound. Compared as parsed
    // `Origin`s, not raw strings — "https://x" and "https://x:443" name the same origin.
    if let Some(base_url) = base_url
        && let Ok(origin) = crate::model::Origin::of(&base_url)
        && !contains_origin(&svc.origin_allowlist, &origin)
    {
        errors.push(ValidationError::Service {
            slug: slug.to_owned(),
            message: format!("base_url's own origin {origin} is not in origin_allowlist"),
        });
    }
}

pub(super) fn contains_origin(allowlist: &BTreeSet<String>, origin: &crate::model::Origin) -> bool {
    allowlist.iter().any(|o| {
        o.parse::<crate::model::Origin>()
            .is_ok_and(|p| &p == origin)
    })
}

fn validate_auth_provider(
    errors: &mut Vec<ValidationError>,
    pack: &Pack,
    slug: &str,
    provider: &PackAuthProvider,
) {
    let Some(service) = pack.services.get(&provider.service) else {
        errors.push(ValidationError::AuthProvider {
            slug: slug.to_owned(),
            message: format!(
                "references service {:?}, which is not in this pack",
                provider.service
            ),
        });
        return;
    };
    match provider.bound_origin.parse::<crate::model::Origin>() {
        Ok(origin) if contains_origin(&service.origin_allowlist, &origin) => {}
        Ok(_) => errors.push(ValidationError::AuthProvider {
            slug: slug.to_owned(),
            message: format!(
                "bound_origin {:?} is not in service {:?}'s origin_allowlist",
                provider.bound_origin, provider.service
            ),
        }),
        Err(e) => errors.push(ValidationError::AuthProvider {
            slug: slug.to_owned(),
            message: format!("bound_origin {:?}: {e}", provider.bound_origin),
        }),
    }
}

fn validate_tag_vocabulary(errors: &mut Vec<ValidationError>, pack: &Pack) {
    let mut union: BTreeSet<&String> = BTreeSet::new();
    for call in pack.api_calls.values() {
        union.extend(call.tags.iter());
    }
    for script in pack.scripts.values() {
        union.extend(script.tags.iter());
    }
    let declared: BTreeSet<&String> = pack.tags.iter().collect();
    if union != declared {
        let missing: Vec<&&String> = union.difference(&declared).collect();
        let extra: Vec<&&String> = declared.difference(&union).collect();
        errors.push(ValidationError::Tags(format!(
            "declared `tags` does not equal the union of every item's own tags (missing: {missing:?}, extra: {extra:?})"
        )));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    pub(super) fn service() -> PackService {
        PackService {
            base_url: "https://svc.example.com/".to_owned(),
            origin_allowlist: BTreeSet::from(["https://svc.example.com".to_owned()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    pub(super) fn empty_pack() -> Pack {
        Pack {
            version: super::super::PACK_VERSION,
            services: BTreeMap::new(),
            auth_providers: BTreeMap::new(),
            api_calls: BTreeMap::new(),
            scripts: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            tags: BTreeSet::new(),
        }
    }

    #[test]
    fn empty_pack_is_valid() {
        assert!(validate(&empty_pack()).is_ok());
    }

    #[test]
    fn wrong_version_is_rejected() {
        let mut pack = empty_pack();
        pack.version = 999;
        let errors = validate(&pack).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnsupportedVersion { .. }))
        );
    }

    #[test]
    fn credential_shaped_value_is_rejected() {
        let mut pack = empty_pack();
        pack.services.insert("svc".to_owned(), service());
        pack.auth_providers.insert(
            "prov".to_owned(),
            PackAuthProvider {
                service: "svc".to_owned(),
                kind: super::super::PackAuthKind::StaticHeader,
                // A live-looking token where an env var *name* belongs.
                credential_env_key: "ghp_aBcDeFgHiJkLmNoPqRsT1234567890".to_owned(),
                header_name: "Authorization".to_owned(),
                value_template: "Bearer {token}".to_owned(),
                scopes: Vec::new(),
                token_url: None,
                bound_origin: "https://svc.example.com".to_owned(),
            },
        );
        let errors = validate(&pack).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::Credential { .. }))
        );
    }
}
