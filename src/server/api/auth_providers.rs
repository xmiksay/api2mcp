//! `GET/POST/PUT/DELETE /api/auth_providers[/{slug}]` — the I5 sharp edge. Every write here
//! reaches `store::auth_provider`'s `pub(crate)` create/update/delete methods directly; what
//! keeps that safe is documented on `super`'s module doc (a bearer service token structurally
//! cannot become a [`Caller`], so it can never reach a handler in this file at all) and on
//! `store::auth_provider`'s own doc.
//!
//! `auth_providers.slug` is unique globally (`store::auth_provider::id_by_slug_global`'s own
//! doc), so this resource is addressed by its bare slug, not `(service, slug)` — [`find`] does
//! the global lookup `AuthProviderStore` doesn't expose directly. Moving a provider to a
//! different service via `PUT` is rejected rather than half-supported: `AuthProviderStore::update`
//! looks the existing row up by `(service_slug, slug)`, so a changed `service` would just miss.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::model::AuthProvider;
use crate::pack::PackAuthProvider;
use crate::server::state::AppState;

use super::convert::{auth_provider_from_pack, auth_provider_to_pack, parse_slug};
use super::dto::{AuthProviderCreate, AuthProviderView};
use super::validate_write::{PendingChange, validate_change};
use super::{ApiError, Caller, require_admin};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth_providers", get(list).post(create))
        .route(
            "/auth_providers/{slug}",
            get(get_one).put(update).delete(remove),
        )
}

fn to_view(p: &AuthProvider) -> AuthProviderView {
    AuthProviderView {
        slug: p.slug.as_str().to_owned(),
        def: auth_provider_to_pack(p),
    }
}

async fn find(state: &AppState, slug: &str) -> Result<AuthProvider, ApiError> {
    let all = state
        .stores()
        .auth_provider()
        .list_all()
        .await
        .map_err(ApiError::from_store)?;
    all.into_iter()
        .find(|p| p.slug.as_str() == slug)
        .ok_or_else(|| ApiError::NotFound(format!("auth provider {slug:?} not found")))
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<AuthProviderView>>, ApiError> {
    require_admin(&caller)?;
    let all = state
        .stores()
        .auth_provider()
        .list_all()
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(all.iter().map(to_view).collect()))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<AuthProviderView>, ApiError> {
    require_admin(&caller)?;
    let provider = find(&state, &slug).await?;
    Ok(Json(to_view(&provider)))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<AuthProviderCreate>,
) -> Result<(StatusCode, Json<AuthProviderView>), ApiError> {
    require_admin(&caller)?;
    let slug = parse_slug(&body.slug).map_err(ApiError::BadRequest)?;
    let service_slug = parse_slug(&body.def.service).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        PendingChange::UpsertAuthProvider(body.slug.clone(), body.def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let provider =
        auth_provider_from_pack(slug, service_slug, &body.def).map_err(ApiError::BadRequest)?;
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
    require_admin(&caller)?;
    let existing = find(&state, &slug).await?;
    if existing.service_slug.as_str() != body.service {
        return Err(ApiError::BadRequest(
            "cannot move an auth provider to a different service via PUT; delete and recreate it instead"
                .to_owned(),
        ));
    }
    validate_change(
        &state.stores(),
        PendingChange::UpsertAuthProvider(slug.clone(), body.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let parsed_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let provider = auth_provider_from_pack(parsed_slug, existing.service_slug, &body)
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
    require_admin(&caller)?;
    let existing = find(&state, &slug).await?;
    validate_change(&state.stores(), PendingChange::RemoveAuthProvider(slug))
        .await
        .map_err(ApiError::Validation)?;
    state
        .stores()
        .auth_provider()
        .delete(&existing.service_slug, &existing.slug)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}
