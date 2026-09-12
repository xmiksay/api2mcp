//! `POST/GET /api/tokens`, `DELETE /api/tokens/{id}` — self-service access-token management,
//! the SPA's answer to "I'm signed in, how do I get a token for my agent" (previously only
//! `api2mcp token mint` on the CLI, `cli::token`'s own doc). Session-only, same as every route
//! in `server::api` (see this module's parent doc) — a service token can never mint, list or
//! revoke a token, because a token that can issue tokens makes revocation meaningless.
//!
//! **Ownership, not admin/non-admin.** Every route here scopes to `caller.id` as the token's
//! `owner_id`. [`revoke`] looks a token up by id *and* checks ownership before touching it —
//! someone else's token id comes back [`ApiError::NotFound`], never
//! [`ApiError::Forbidden`], so a caller probing ids learns nothing about which ones exist for
//! another user.
//!
//! **The plaintext appears exactly once**, in [`create`]'s own response — see
//! `store::service_token`'s module doc. Nothing here logs it, and [`TokenView`] (used by
//! [`list`]) has no field to put it in, mirroring `store::ServiceTokenRecord`'s own shape.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::model::Slug;
use crate::server::state::AppState;
use crate::store::ServiceTokenRecord;

use super::convert::parse_slug;
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tokens", get(list).post(create))
        .route("/tokens/{id}", delete(revoke))
}

#[derive(Debug, Deserialize)]
struct TokenCreate {
    label: String,
    expires_in_days: Option<i64>,
    #[serde(default)]
    endpoints: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TokenMinted {
    id: Uuid,
    token: String,
    token_prefix: String,
    label: String,
    expires_at: Option<String>,
    endpoints: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TokenView {
    id: Uuid,
    token_prefix: String,
    label: String,
    created_at: String,
    last_used_at: Option<String>,
    expires_at: Option<String>,
    revoked_at: Option<String>,
    endpoints: Vec<String>,
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339()
}

fn endpoint_slugs(endpoints: &BTreeSet<Slug>) -> Vec<String> {
    endpoints.iter().map(|s| s.as_str().to_owned()).collect()
}

fn to_view(record: ServiceTokenRecord) -> TokenView {
    TokenView {
        id: record.id,
        token_prefix: record.token_prefix,
        label: record.label,
        created_at: rfc3339(record.created_at),
        last_used_at: record.last_used_at.map(rfc3339),
        expires_at: record.expires_at.map(rfc3339),
        revoked_at: record.revoked_at.map(rfc3339),
        endpoints: endpoint_slugs(&record.endpoints),
    }
}

/// Parses the wire `endpoints` slug list into the `BTreeSet<Slug>` the store wants. A slug
/// naming no real endpoint is caught by the store itself (`ServiceTokenStore::mint`, which
/// maps it to [`crate::store::StoreError::Conflict`] -> [`ApiError::BadRequest`]) — this only
/// rejects a string that isn't even syntactically a slug.
fn parse_endpoints(raw: &[String]) -> Result<BTreeSet<Slug>, ApiError> {
    raw.iter()
        .map(|s| parse_slug(s).map_err(ApiError::BadRequest))
        .collect()
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<TokenCreate>,
) -> Result<(StatusCode, Json<TokenMinted>), ApiError> {
    let expires_at = match body.expires_in_days {
        None => None,
        Some(days) if days > 0 => {
            let delta = Duration::try_days(days)
                .ok_or_else(|| ApiError::BadRequest("expires_in_days is out of range".into()))?;
            Some(Utc::now() + delta)
        }
        Some(_) => {
            return Err(ApiError::BadRequest(
                "expires_in_days must be a positive number of days".into(),
            ));
        }
    };
    let endpoints = parse_endpoints(&body.endpoints)?;

    let minted = state
        .stores()
        .service_token()
        .mint(caller.id, body.label, expires_at, endpoints)
        .await
        .map_err(ApiError::from_store)?;

    Ok((
        StatusCode::CREATED,
        Json(TokenMinted {
            id: minted.record.id,
            token: minted.plaintext,
            token_prefix: minted.record.token_prefix,
            label: minted.record.label,
            expires_at: minted.record.expires_at.map(rfc3339),
            endpoints: endpoint_slugs(&minted.record.endpoints),
        }),
    ))
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<TokenView>>, ApiError> {
    let tokens = state
        .stores()
        .service_token()
        .list_for_owner(caller.id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(tokens.into_iter().map(to_view).collect()))
}

async fn revoke(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let store = state.stores().service_token();
    let record = store
        .get_by_id(id)
        .await
        .map_err(ApiError::from_store)?
        .filter(|r| r.owner_id == caller.id)
        .ok_or_else(|| ApiError::NotFound("not found".into()))?;
    store
        .revoke(record.id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}
