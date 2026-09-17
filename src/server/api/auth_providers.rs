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
//! service_slug, slug)`, so a changed `service` would just miss.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use uuid::Uuid;

use crate::model::AuthProvider;
use crate::pack::PackAuthProvider;
use crate::server::state::AppState;

use super::convert::{auth_provider_from_pack, auth_provider_to_pack, parse_slug};
use super::dto::{AuthProviderCreate, AuthProviderView};
use super::validate_write::{PendingChange, validate_change};
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
    }
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
    let service_slug = parse_slug(&body.def.service).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertAuthProvider(body.slug.clone(), body.def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let provider = auth_provider_from_pack(caller.id, slug, service_slug, &body.def)
        .map_err(ApiError::BadRequest)?;
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
    Json(body): Json<PackAuthProvider>,
) -> Result<Json<AuthProviderView>, ApiError> {
    let existing = find(&state, caller.id, &slug).await?;
    if existing.service_slug.as_str() != body.service {
        return Err(ApiError::BadRequest(
            "cannot move an auth provider to a different service via PUT; delete and recreate it instead"
                .to_owned(),
        ));
    }
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertAuthProvider(slug.clone(), body.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let parsed_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let provider = auth_provider_from_pack(caller.id, parsed_slug, existing.service_slug, &body)
        .map_err(ApiError::BadRequest)?;
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
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::RemoveAuthProvider(slug),
    )
    .await
    .map_err(ApiError::Validation)?;
    state
        .stores()
        .auth_provider()
        .delete(caller.id, &existing.service_slug, &existing.slug)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}
