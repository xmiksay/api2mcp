//! `GET/POST/PUT/DELETE /api/auth_providers[/{slug}]` — the I5 sharp edge. Every write here
//! reaches `store::auth_provider`'s `pub(crate)` create/update/delete methods directly; what
//! keeps that safe is documented on `super`'s module doc (a bearer service token structurally
//! cannot become a [`Caller`], so it can never reach a handler in this file at all) and on
//! `store::auth_provider`'s own doc.
//!
//! `auth_providers.slug` is unique per owner (`store::auth_provider::id_by_slug_for_owner`'s own
//! doc), so this resource is addressed by its bare slug within the caller's own scope, not
//! `(service, slug)` — [`find`] does the owner-scoped lookup `AuthProviderStore` doesn't expose
//! directly. Moving a provider to a different service via `PUT` is rejected rather than
//! half-supported: `AuthProviderStore::update` looks the existing row up by `(owner_id,
//! service_slug, slug)`, so a changed `service` would just miss. Creating a second provider on a
//! service that already has one is rejected by `AuthProviderStore::create`'s own `Conflict`
//! check, backed by `ux_auth_providers_service`.
//!
//! **This is the one resource that does not go through `validate_write::validate_change`** — a
//! pack carries no auth providers at all (see `pack`'s own module doc), so there is no
//! `Pack::auth_providers` field left for that pipeline to edit or validate.
//! [`bound_origin_ok`]/[`credential_shape_ok`] are this route's own, narrower replacements for
//! the two checks that pipeline used to make on a provider's behalf: `bound_origin` must be
//! inside its *live* service's `origin_allowlist` (fetched fresh from the database rather than
//! reconstructed into a `Pack`), and `credential_env_key`/`value_template` must not look like a
//! live credential value.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use uuid::Uuid;

use crate::model::{AuthProvider, CredentialSource, Service};
use crate::pack::PackAuthProvider;
use crate::pack::validate_credentials::looks_like_credential;
use crate::secret::Secret;
use crate::server::state::AppState;

use super::convert::{auth_provider_from_pack, auth_provider_to_pack, parse_slug};
use super::dto::{AuthProviderCreate, AuthProviderUpdate, AuthProviderView};
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth_providers", get(list).post(create))
        .route(
            "/auth_providers/{slug}",
            get(get_one).put(update).delete(remove),
        )
}

pub(crate) fn to_view(p: &AuthProvider) -> AuthProviderView {
    AuthProviderView {
        slug: p.slug.as_str().to_owned(),
        def: auth_provider_to_pack(p),
        has_stored_credential: matches!(p.credential, CredentialSource::Stored(Some(_))),
    }
}

/// I5's UX-level pre-write check, replacing what `validate_write::validate_change` used to do
/// for a provider before packs stopped carrying them (see this module's own doc). The
/// authoritative gate is still `resolve::auth_bind` at plan-build time; this exists only to
/// report the same problem earlier, at the point a human actually sets `bound_origin`.
fn bound_origin_ok(service: &Service, provider: &AuthProvider) -> Result<(), ApiError> {
    if service.origin_allowlist.contains(&provider.bound_origin) {
        Ok(())
    } else {
        Err(ApiError::Validation(vec![format!(
            "auth_providers.{}: bound_origin {} is not in service {:?}'s origin_allowlist",
            provider.slug.as_str(),
            provider.bound_origin,
            service.slug.as_str()
        )]))
    }
}

/// The heuristic backstop `pack::validate_credentials::scan` used to run over
/// `Pack::auth_providers` before Change 2 removed that map — a human can still paste a live
/// token where `credential_env_key` (an env var *name*) or `value_template` (a header
/// *template*) belongs, so this route checks the same two fields itself now, directly, with no
/// `Pack` round trip in between.
fn credential_shape_ok(def: &PackAuthProvider) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    if let Some(key) = &def.credential_env_key
        && looks_like_credential(key)
    {
        errors.push(
            "credential_env_key looks like it contains a credential value, not a reference to \
             one"
            .to_owned(),
        );
    }
    if looks_like_credential(&def.value_template) {
        errors.push(
            "value_template looks like it contains a credential value, not a template".to_owned(),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Validation(errors))
    }
}

/// Folds a request's write-only `credential_value` into a provider whose source
/// [`auth_provider_from_pack`] has already decided. An env-backed provider ignores the field
/// entirely rather than silently becoming stored-source: which source a provider uses is
/// determined by `credential_env_key` alone, in one place.
fn apply_credential_value(
    provider: &mut AuthProvider,
    submitted: Option<String>,
    existing: Option<&AuthProvider>,
) {
    if !matches!(provider.credential, CredentialSource::Stored(_)) {
        return;
    }
    let carried_over = existing.and_then(|e| match &e.credential {
        CredentialSource::Stored(value) => value.clone(),
        CredentialSource::Env(_) => None,
    });
    provider.credential = CredentialSource::Stored(match submitted {
        // An explicit empty string clears the stored value; absent leaves it as it was.
        Some(value) if value.is_empty() => None,
        Some(value) => Some(Secret::from_raw(value)),
        None => carried_over,
    });
}

/// `pub(crate)`, not private: `server::mcp::control::auth_providers` (the MCP control-plane's
/// read-only auth-provider tools — I5 forbids write access, never read) calls this and
/// [`list_for_owner`] directly rather than duplicating the lookup.
pub(crate) async fn find(
    state: &AppState,
    owner_id: Uuid,
    slug: &str,
) -> Result<AuthProvider, ApiError> {
    let all = state
        .stores()
        .auth_provider()
        .list_all(owner_id)
        .await
        .map_err(ApiError::from_store)?;
    all.into_iter()
        .find(|p| p.slug.as_str() == slug)
        .ok_or_else(|| ApiError::NotFound(format!("auth provider {slug:?} not found")))
}

pub(crate) async fn list_for_owner(
    state: &AppState,
    owner_id: Uuid,
) -> Result<Vec<AuthProviderView>, ApiError> {
    let all = state
        .stores()
        .auth_provider()
        .list_all(owner_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(all.iter().map(to_view).collect())
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<AuthProviderView>>, ApiError> {
    Ok(Json(list_for_owner(&state, caller.id).await?))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<AuthProviderView>, ApiError> {
    let provider = find(&state, caller.id, &slug).await?;
    Ok(Json(to_view(&provider)))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<AuthProviderCreate>,
) -> Result<(StatusCode, Json<AuthProviderView>), ApiError> {
    let slug = parse_slug(&body.slug).map_err(ApiError::BadRequest)?;
    credential_shape_ok(&body.def)?;
    let service_slug = parse_slug(&body.def.service).map_err(ApiError::BadRequest)?;
    let service = state
        .stores()
        .service()
        .get_by_slug(caller.id, &service_slug)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| {
            ApiError::Validation(vec![format!(
                "auth_providers.{}: references service {:?}, which does not exist",
                body.slug, body.def.service
            )])
        })?;
    let mut provider = auth_provider_from_pack(caller.id, slug, service_slug, &body.def)
        .map_err(ApiError::BadRequest)?;
    bound_origin_ok(&service, &provider)?;
    apply_credential_value(&mut provider, body.credential_value, None);
    state
        .stores()
        .auth_provider()
        .create(&provider)
        .await
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, Json(to_view(&provider))))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<AuthProviderUpdate>,
) -> Result<Json<AuthProviderView>, ApiError> {
    let existing = find(&state, caller.id, &slug).await?;
    if existing.service_slug.as_str() != body.def.service {
        return Err(ApiError::BadRequest(
            "cannot move an auth provider to a different service via PUT; delete and recreate it instead"
                .to_owned(),
        ));
    }
    credential_shape_ok(&body.def)?;
    let service = state
        .stores()
        .service()
        .get_by_slug(caller.id, &existing.service_slug)
        .await
        .map_err(ApiError::from_store)?
        .ok_or(ApiError::Internal)?;
    let parsed_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let mut provider = auth_provider_from_pack(
        caller.id,
        parsed_slug,
        existing.service_slug.clone(),
        &body.def,
    )
    .map_err(ApiError::BadRequest)?;
    bound_origin_ok(&service, &provider)?;
    apply_credential_value(&mut provider, body.credential_value, Some(&existing));
    state
        .stores()
        .auth_provider()
        .update(&provider)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(to_view(&provider)))
}

async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let existing = find(&state, caller.id, &slug).await?;
    // No pre-write revalidation needed: `endpoint_auth_providers.auth_provider_id` and
    // `api_calls` reference a provider only through its *service*, both `ON DELETE CASCADE`/
    // resolved live, never through a value that could go stale — see this module's own doc.
    state
        .stores()
        .auth_provider()
        .delete(caller.id, &existing.service_slug, &existing.slug)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}
