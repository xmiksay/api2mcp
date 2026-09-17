//! `GET/POST/PUT/DELETE /api/endpoints[/{slug}]` and `GET /api/endpoints/{slug}/plan` — the one
//! screen that shows a human something the YAML/DB rows can't: the resolved tool list with
//! generated schemas, plus the statically computed reachable-origin set (I2).

use serde::Serialize;
use serde_json::Value;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use uuid::Uuid;

use crate::model::{Budgets, EndpointDef};
use crate::pack::PackEndpoint;
use crate::resolve::plan::ToolTarget;
use crate::server::state::AppState;

use super::convert::{access_to_str, parse_slug};
use super::convert_items::{endpoint_from_pack, endpoint_to_pack};
use super::dto::{EndpointCreate, EndpointView};
use super::test_run::resolve_plan;
use super::validate_write::{PendingChange, validate_change};
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/endpoints", get(list).post(create))
        .route("/endpoints/{slug}", get(get_one).put(update).delete(remove))
        .route("/endpoints/{slug}/plan", get(plan))
}

fn to_view(e: &EndpointDef) -> EndpointView {
    EndpointView {
        slug: e.slug.as_str().to_owned(),
        def: endpoint_to_pack(e),
    }
}

// See `services.rs`'s own comment: the `_for_owner` functions below are the real logic, reused
// as-is by `server::mcp::control::endpoints`; the axum handlers are thin shims.

pub(crate) async fn list_for_owner(
    state: &AppState,
    owner_id: Uuid,
) -> Result<Vec<EndpointView>, ApiError> {
    let all = state
        .stores()
        .endpoint()
        .list_all(owner_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(all.iter().map(to_view).collect())
}

pub(crate) async fn get_by_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: &str,
) -> Result<EndpointView, ApiError> {
    let parsed = parse_slug(slug).map_err(ApiError::BadRequest)?;
    let endpoint = state
        .stores()
        .endpoint()
        .get(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("endpoint {slug:?} not found")))?;
    Ok(to_view(&endpoint))
}

pub(crate) async fn create_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackEndpoint,
) -> Result<EndpointView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertEndpoint(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let endpoint = endpoint_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .endpoint()
        .create(&endpoint)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&endpoint))
}

pub(crate) async fn update_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: String,
    def: PackEndpoint,
) -> Result<EndpointView, ApiError> {
    let parsed = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    validate_change(
        &state.stores(),
        owner_id,
        PendingChange::UpsertEndpoint(slug, def.clone()),
    )
    .await
    .map_err(ApiError::Validation)?;
    let endpoint = endpoint_from_pack(owner_id, parsed, &def).map_err(ApiError::BadRequest)?;
    state
        .stores()
        .endpoint()
        .update(&endpoint)
        .await
        .map_err(ApiError::from_store)?;
    Ok(to_view(&endpoint))
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
        PendingChange::RemoveEndpoint(slug),
    )
    .await
    .map_err(ApiError::Validation)?;
    state
        .stores()
        .endpoint()
        .delete(owner_id, &parsed)
        .await
        .map_err(ApiError::from_store)?;
    Ok(())
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<EndpointView>>, ApiError> {
    Ok(Json(list_for_owner(&state, caller.id).await?))
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<EndpointView>, ApiError> {
    Ok(Json(get_by_owner(&state, caller.id, &slug).await?))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<EndpointCreate>,
) -> Result<(StatusCode, Json<EndpointView>), ApiError> {
    let view = create_for_owner(&state, caller.id, body.slug, body.def).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
    Json(body): Json<PackEndpoint>,
) -> Result<Json<EndpointView>, ApiError> {
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

#[derive(Debug, Serialize)]
struct BudgetsView {
    max_calls: Option<u32>,
    max_bytes: Option<u64>,
    wall_clock_ms: Option<u64>,
    max_pages: Option<u32>,
    max_concurrency: Option<u32>,
}

fn budgets_view(b: &Budgets) -> BudgetsView {
    BudgetsView {
        max_calls: b.max_calls,
        max_bytes: b.max_bytes,
        wall_clock_ms: b.wall_clock.map(|d| d.as_millis() as u64),
        max_pages: b.max_pages,
        max_concurrency: b.max_concurrency,
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ToolView {
    name: String,
    input_schema: Value,
    target_kind: &'static str,
    target_slug: String,
    budgets: BudgetsView,
}

#[derive(Debug, Serialize)]
pub(crate) struct PlanView {
    slug: String,
    write_ceiling: &'static str,
    instructions: Option<String>,
    digest: String,
    budgets: BudgetsView,
    /// The statically computed reachable-origin set (I2) — the whole reason this route exists:
    /// it shows something no YAML/DB row can, by itself, tell a human.
    origins: Vec<String>,
    tools: Vec<ToolView>,
}

/// The real logic behind `plan`, reused by `server::mcp::control::endpoints`'s `endpoint.plan`
/// tool — see `services.rs`'s own comment.
pub(crate) async fn plan_for_owner(
    state: &AppState,
    owner_id: Uuid,
    slug: &str,
) -> Result<PlanView, ApiError> {
    let endpoint_slug = parse_slug(slug).map_err(ApiError::BadRequest)?;
    let plan = resolve_plan(state, owner_id, &endpoint_slug).await?;

    let tools = plan
        .tools
        .iter()
        .map(|t| {
            let (target_kind, target_slug) = match &t.target {
                ToolTarget::ApiCall(s) => ("api_call", s.as_str().to_owned()),
                ToolTarget::Script(s) => ("script", s.as_str().to_owned()),
            };
            ToolView {
                name: t.name.clone(),
                input_schema: t.input_schema.clone(),
                target_kind,
                target_slug,
                budgets: budgets_view(&t.budgets),
            }
        })
        .collect();

    Ok(PlanView {
        slug: plan.slug.as_str().to_owned(),
        write_ceiling: access_to_str(plan.write_ceiling),
        instructions: plan.instructions.clone(),
        digest: plan.digest.clone(),
        budgets: budgets_view(&plan.budgets),
        origins: plan.origins.iter().map(|o| o.to_string()).collect(),
        tools,
    })
}

async fn plan(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Json<PlanView>, ApiError> {
    Ok(Json(plan_for_owner(&state, caller.id, &slug).await?))
}
