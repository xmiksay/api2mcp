//! `GET/POST/PUT/DELETE /api/services[/{slug}]`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

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

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<ServiceView>>, ApiError> {
    let services = state
        .stores()
        .service()
        .list(caller.id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(services.iter().map(to_view).collect()))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<ServiceView>, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let svc = state
        .stores()
        .service()
        .get_by_slug(caller.id, &parsed)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("service {slug:?} not found")))?;
    Ok(Json(to_view(&svc)))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ServiceCreate>,
) -> Result<(StatusCode, Json<ServiceView>), ApiError> {
    let parsed = parse_slug(&body.slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertService(body.slug.clone(), body.def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let service = service_from_pack(caller.id, parsed, &body.def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .service()
        .create(&service)
        .await
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, Json(to_view(&service))))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackService>,
) -> Result<Json<ServiceView>, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertService(slug.clone(), body.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let service = service_from_pack(caller.id, parsed, &body).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .service()
        .update(&service)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(to_view(&service)))
}

async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::RemoveService(slug),
    )
    .await
    .map_err(ApiError::Validation)?;
    state
        .stores()
        .service()
        .delete(caller.id, &parsed)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}
