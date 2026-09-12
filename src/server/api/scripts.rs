//! `GET/POST/PUT/DELETE /api/scripts[/{slug}]` and `POST /api/scripts/{slug}/test`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{ScriptDef, Tag};
use crate::pack::PackScript;
use crate::server::state::AppState;

use std::collections::BTreeSet;

use super::convert::{parse_slug, tags_from_pack};
use super::convert_items::{script_from_pack, script_to_pack};
use super::dto::{ScriptCreate, ScriptView};
use super::test_run::{ScriptTestResult, run_script_test};
use super::validate_write::{PendingChange, validate_change};
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scripts", get(list).post(create))
        .route("/scripts/{slug}", get(get_one).put(update).delete(remove))
        .route("/scripts/{slug}/test", post(test))
}

fn to_view(s: &ScriptDef, tags: &BTreeSet<Tag>) -> ScriptView {
    ScriptView {
        slug: s.slug.as_str().to_owned(),
        def: script_to_pack(s, tags),
    }
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<ScriptView>>, ApiError> {
    let all = state
        .stores()
        .script()
        .list_all(caller.id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(
        all.iter().map(|t| to_view(&t.script, &t.tags)).collect(),
    ))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<ScriptView>, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let t = state
        .stores()
        .script()
        .get(caller.id, &parsed)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("script {slug:?} not found")))?;
    Ok(Json(to_view(&t.script, &t.tags)))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ScriptCreate>,
) -> Result<(StatusCode, Json<ScriptView>), ApiError> {
    let slug = parse_slug(&body.slug).map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&body.def.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertScript(body.slug.clone(), body.def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let script = script_from_pack(caller.id, slug, &body.def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .script()
        .create(&script, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::CREATED, Json(to_view(&script, &tags))))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackScript>,
) -> Result<Json<ScriptView>, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&body.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        caller.id,
        PendingChange::UpsertScript(slug.clone(), body.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let script = script_from_pack(caller.id, parsed, &body).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .script()
        .update(&script, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(to_view(&script, &tags)))
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
        PendingChange::RemoveScript(slug),
    )
    .await
    .map_err(ApiError::Validation)?;
    state
        .stores()
        .script()
        .delete(caller.id, &parsed)
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
) -> Result<Json<ScriptTestResult>, ApiError> {
    let script_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let endpoint_slug = parse_slug(&body.endpoint).map_err(ApiError::BadRequest)?;
    let result = run_script_test(&state, &endpoint_slug, &script_slug, body.args, &caller).await?;
    Ok(Json(result))
}
