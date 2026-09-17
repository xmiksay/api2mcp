//! `GET/POST/PUT/DELETE /api/scripts[/{slug}]` and `POST /api/scripts/{slug}/test`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

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

// See `services.rs`'s own comment: the `_for_owner` functions below are the real logic, reused
// as-is by `server::mcp::control::scripts`; the axum handlers are thin shims.

pub(crate) async fn list_for_owner(
    state: &AppState,
    owner_id: Uuid,
) -> Result<Vec<ScriptView>, ApiError> {
    let all = state
        .stores()
        .script()
        .list_all(owner_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(all.iter().map(|t| to_view(&t.script, &t.tags)).collect())
}

pub(crate) async fn get_by_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: &str,
) -> Result<ScriptView, ApiError> {
    let parsed = parse_slug(slug).map_err(ApiError::BadRequest)?;
    let t = state
        .stores()
        .script()
        .get(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("script {slug:?} not found")))?;
    Ok(to_view(&t.script, &t.tags))
}

pub(crate) async fn create_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackScript,
) -> Result<ScriptView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&def.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertScript(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let script = script_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .script()
        .create(&script, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&script, &tags))
}

pub(crate) async fn update_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackScript,
) -> Result<ScriptView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let tags = tags_from_pack(&def.tags).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertScript(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let script = script_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .script()
        .update(&script, &tags)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&script, &tags))
}

pub(crate) async fn delete_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
) -> Result<(), ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(&state.stores(), owner_id, PendingChange::RemoveScript(slug))
        .await
        .map_err(ApiError::Validation)?;
    state
        .stores()
        .script()
        .delete(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?;
    Ok(())
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<ScriptView>>, ApiError> {
    Ok(Json(list_for_owner(&state, caller.id).await?))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<ScriptView>, ApiError> {
    Ok(Json(get_by_owner(&state, caller.id, &slug).await?))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ScriptCreate>,
) -> Result<(StatusCode, Json<ScriptView>), ApiError> {
    let view = create_for_owner(&state, caller.id, body.slug, body.def).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackScript>,
) -> Result<Json<ScriptView>, ApiError> {
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
