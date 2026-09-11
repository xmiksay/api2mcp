//! `GET/POST/PUT/DELETE /api/api_calls[/{slug}]` and `POST /api/api_calls/{slug}/test`.
//!
//! `api_calls.slug` is unique globally (`store::api_call::id_by_slug_global`'s own doc), so this
//! resource is addressed by its bare slug — [`find`] does the lookup `ApiCallStore` doesn't
//! expose as a single query. Moving an api_call to a different service via `PUT` is rejected for
//! the same reason `auth_providers.rs` rejects it: `ApiCallStore::update` looks the existing row
//! up by `(service_slug, slug)`.

use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{ApiCall, Tag};
use crate::pack::PackApiCall;
use crate::server::state::AppState;
use crate::store::TaggedApiCall;

use super::convert::{parse_slug, tags_from_pack};
use super::convert_items::{api_call_from_pack, api_call_to_pack};
use super::dto::{ApiCallCreate, ApiCallView};
use super::test_run::{ApiCallTestResult, run_api_call_test};
use super::validate_write::{PendingChange, validate_change};
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api_calls", get(list).post(create))
        .route("/api_calls/{slug}", get(get_one).put(update).delete(remove))
        .route("/api_calls/{slug}/test", post(test))
}

fn to_view(c: &ApiCall, tags: &BTreeSet<Tag>) -> ApiCallView {
    ApiCallView {
        slug: c.slug.as_str().to_owned(),
        def: api_call_to_pack(c, tags),
    }
}

async fn find(state: &AppState, slug: &str) -> Result<TaggedApiCall, ApiError> {
    let all = state
        .stores()
        .api_call()
        .list_all()
        .await
        .map_err(ApiError::from_store)?;
    all.into_iter()
        .find(|t| t.api_call.slug.as_str() == slug)
        .ok_or_else(|| ApiError::NotFound(format!("api_call {slug:?} not found")))
}

async fn list(
    State(state): State<AppState>,
    _caller: Caller,
) -> Result<Json<Vec<ApiCallView>>, ApiError> {
    let all = state
        .stores()
        .api_call()
        .list_all()
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(
        all.iter().map(|t| to_view(&t.api_call, &t.tags)).collect(),
    ))
}

async fn get_one(
    State(state): State<AppState>,
    _caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<ApiCallView>, ApiError> {
    let t = find(&state, &slug).await?;
    Ok(Json(to_view(&t.api_call, &t.tags)))
}

async fn create(
    State(state): State<AppState>,
    _caller: Caller,
    Json(body): Json<ApiCallCreate>,
) -> Result<(StatusCode, Json<ApiCallView>), ApiError> {
    let slug = parse_slug(&body.slug).map_err(ApiError::BadRequest)?;
    let service_slug = parse_slug(&body.def.service).map_err(ApiError::BadRequest)?;
    let auth_provider_slug = body
        .def
        .auth_provider
        .as_deref()
        .map(parse_slug)
        .transpose()
        .map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&body.def.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        PendingChange::UpsertApiCall(body.slug.clone(), body.def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let call = api_call_from_pack(slug, service_slug, auth_provider_slug, &body.def)
        .map_err(ApiError::BadRequest)?;
    state
        .stores()
        .api_call()
        .create(&call, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, Json(to_view(&call, &tags))))
}

async fn update(
    State(state): State<AppState>,
    _caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackApiCall>,
) -> Result<Json<ApiCallView>, ApiError> {
    let existing = find(&state, &slug).await?;
    if existing.api_call.service_slug.as_str() != body.service {
        return Err(ApiError::BadRequest(
            "cannot move an api_call to a different service via PUT; delete and recreate it instead"
                .to_owned(),
        ));
    }
    let auth_provider_slug = body
        .auth_provider
        .as_deref()
        .map(parse_slug)
        .transpose()
        .map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&body.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        PendingChange::UpsertApiCall(slug.clone(), body.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let parsed_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let call = api_call_from_pack(
        parsed_slug,
        existing.api_call.service_slug,
        auth_provider_slug,
        &body,
    )
    .map_err(ApiError::BadRequest)?;
    state
        .stores()
        .api_call()
        .update(&call, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(to_view(&call, &tags)))
}

async fn remove(
    State(state): State<AppState>,
    _caller: Caller,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let existing = find(&state, &slug).await?;
    validate_change(&state.stores(), PendingChange::RemoveApiCall(slug))
        .await
        .map_err(ApiError::Validation)?;
    state
        .stores()
        .api_call()
        .delete(&existing.api_call.service_slug, &existing.api_call.slug)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct TestBody {
    args: Value,
    endpoint: String,
}

async fn test(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<TestBody>,
) -> Result<Json<ApiCallTestResult>, ApiError> {
    let api_call_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let endpoint_slug = parse_slug(&body.endpoint).map_err(ApiError::BadRequest)?;
    let result =
        run_api_call_test(&state, &endpoint_slug, &api_call_slug, body.args, &caller).await?;
    Ok(Json(result))
}
