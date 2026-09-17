//! `GET/POST/PUT/DELETE /api/services[/{slug}]`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use uuid::Uuid;

use crate::model::Service;
use crate::pack::PackService;
use crate::server::state::AppState;

use super::convert::{parse_slug, service_from_pack, service_to_pack};
use super::dto::{ServiceCreate, ServiceView};
use super::validate_write::{PendingChange, validate_change};
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services", get(list).post(create))
        .route("/services/{slug}", get(get_one).put(update).delete(remove))
}

fn to_view(s: &Service) -> ServiceView {
    ServiceView {
        slug: s.slug.as_str().to_owned(),
        def: service_to_pack(s),
    }
}

// The `_for_owner`/`_by_owner` functions below are the actual logic; the axum handlers
// following them are one-line extraction shims. `server::mcp::control::services` (the MCP
// control-plane tool surface) calls these same functions directly with the tool-calling
// token's own owner id — see this crate's module doc on reuse over duplication.

pub(crate) async fn list_for_owner(
    state: &AppState,
    owner_id: Uuid,
) -> Result<Vec<ServiceView>, ApiError> {
    let services = state
        .stores()
        .service()
        .list(owner_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(services.iter().map(to_view).collect())
}

pub(crate) async fn get_by_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: &str,
) -> Result<ServiceView, ApiError> {
    let parsed = parse_slug(slug).map_err(ApiError::BadRequest)?;
    let svc = state
        .stores()
        .service()
        .get_by_slug(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("service {slug:?} not found")))?;
    Ok(to_view(&svc))
}

pub(crate) async fn create_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackService,
) -> Result<ServiceView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertService(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let service = service_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .service()
        .create(&service)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&service))
}

pub(crate) async fn update_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackService,
) -> Result<ServiceView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertService(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let service = service_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .service()
        .update(&service)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&service))
}

pub(crate) async fn delete_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
) -> Result<(), ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::RemoveService(slug),
    )
    .await
    .map_err(ApiError::Validation)?;
    state
        .stores()
        .service()
        .delete(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?;
    Ok(())
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<ServiceView>>, ApiError> {
    Ok(Json(list_for_owner(&state, caller.id).await?))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<ServiceView>, ApiError> {
    Ok(Json(get_by_owner(&state, caller.id, &slug).await?))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ServiceCreate>,
) -> Result<(StatusCode, Json<ServiceView>), ApiError> {
    let view = create_for_owner(&state, caller.id, body.slug, body.def).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackService>,
) -> Result<Json<ServiceView>, ApiError> {
    Ok(Json(update_for_owner(&state, caller.id, slug, body).await?))
}

async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    delete_for_owner(&state, caller.id, slug).await?;
    Ok(StatusCode::NO_CONTENT)
}
