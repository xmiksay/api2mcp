//! `GET /api/endpoints/{slug}/pack` and `POST /api/packs/import` — pack export/import over the
//! HTTP API, sharing the exact same [`crate::pack::export_endpoint`]/[`crate::pack::validate`]/
//! [`crate::pack::import`] functions `api2mcp export`/`api2mcp import` call on the CLI: one pack
//! format, two entry points (see `pack`'s own module doc for why that format carries no auth
//! providers at all). Both routes are session-only and owner-scoped like every other route in
//! this module.
//!
//! `POST /api/packs/import` always runs [`crate::pack::validate`] first and reports **every**
//! failure it finds as a single [`ApiError::Validation`], never just the first — the same
//! all-at-once contract the CLI gets. `?dry_run=true` additionally classifies every row
//! (create/update/unchanged) against the caller's own existing definitions without writing
//! anything, so a UI can show a full preview — problems and all — before committing to anything.

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::pack::{ExportError, ImportChange, ImportReport, Pack};
use crate::server::state::AppState;

use super::convert::parse_slug;
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/endpoints/{slug}/pack", get(export_pack))
        .route("/packs/import", post(import_pack))
}

async fn export_pack(
    State(state): State<AppState>,
    caller: Caller,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    let endpoint_slug = parse_slug(&slug).map_err(ApiError::BadRequest)?;
    let pack = crate::pack::export_endpoint(&state.stores(), caller.id, &endpoint_slug)
        .await
        .map_err(|e| match e {
            ExportError::EndpointNotFound(_) => ApiError::NotFound(e.to_string()),
            other => ApiError::BadRequest(other.to_string()),
        })?;
    let yaml = serde_norway::to_string(&pack).map_err(|_| ApiError::Internal)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
        yaml,
    )
        .into_response())
}

#[derive(Debug, Default, Deserialize)]
struct ImportQuery {
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct ImportRowView {
    slug: String,
    change: &'static str,
}

#[derive(Debug, Serialize)]
struct ImportReportView {
    dry_run: bool,
    services: Vec<ImportRowView>,
    api_calls: Vec<ImportRowView>,
    scripts: Vec<ImportRowView>,
    endpoints: Vec<ImportRowView>,
}

fn view_rows(rows: &[(String, ImportChange)]) -> Vec<ImportRowView> {
    rows.iter()
        .map(|(slug, change)| ImportRowView {
            slug: slug.clone(),
            change: match change {
                ImportChange::Created => "created",
                ImportChange::Updated => "updated",
                ImportChange::Unchanged => "unchanged",
            },
        })
        .collect()
}

fn view_report(dry_run: bool, r: &ImportReport) -> ImportReportView {
    ImportReportView {
        dry_run,
        services: view_rows(&r.services),
        api_calls: view_rows(&r.api_calls),
        scripts: view_rows(&r.scripts),
        endpoints: view_rows(&r.endpoints),
    }
}

/// Accepts a pack as a raw YAML body — the same shape [`export_pack`] hands back, so
/// "download, then re-upload elsewhere" needs no reformatting. `body` must be the last
/// extractor (it consumes the request); `Query`/`Caller` above it only read the URI/headers.
async fn import_pack(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<ImportQuery>,
    body: String,
) -> Result<Json<ImportReportView>, ApiError> {
    let pack: Pack = serde_norway::from_str(&body)
        .map_err(|e| ApiError::BadRequest(format!("parsing request body as a pack: {e}")))?;
    crate::pack::validate(&pack)
        .map_err(|errors| ApiError::Validation(errors.iter().map(|e| e.to_string()).collect()))?;
    let report = crate::pack::import(&state.stores(), &pack, query.dry_run, caller.id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(view_report(query.dry_run, &report)))
}
