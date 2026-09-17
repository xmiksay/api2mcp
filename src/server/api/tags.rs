//! `GET /api/tags` — the tag vocabulary, read-only (tags have no independent identity to write;
//! they're created implicitly by whatever api_call/script first uses them — see `store::tag`).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::server::state::AppState;

use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new().route("/tags", get(list))
}

async fn list(
    State(state): State<AppState>,
    _caller: Caller,
) -> Result<Json<Vec<String>>, ApiError> {
    let tags = state
        .stores()
        .tag()
        .list()
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(tags.into_iter().map(|t| t.0.into_string()).collect()))
}
