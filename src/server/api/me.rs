//! `GET /api/me` — who the caller is. Trivial, but it is what the admin SPA uses to confirm a
//! session is live and to render the signed-in identity, so it earns its own tiny route.
//!
//! It returns the caller's **email**, not just an id and a kind. The kind answers "how did you
//! authenticate", which is a fact about the credential and not about the person; a header that
//! renders it reads "signed in as session", which tells nobody anything. The email is the only
//! field here that identifies the human.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use uuid::Uuid;

use crate::server::identity::CallerKind;
use crate::server::state::AppState;

use super::{ApiError, Caller};

#[derive(Debug, Serialize)]
pub struct MeView {
    pub id: Uuid,
    pub email: String,
    pub kind: &'static str,
}

fn kind_str(kind: CallerKind) -> &'static str {
    match kind {
        CallerKind::Session => "session",
        CallerKind::Oauth => "oauth",
        CallerKind::ServiceToken => "service_token",
        CallerKind::Cli => "cli",
    }
}

pub async fn me(State(state): State<AppState>, caller: Caller) -> Result<Json<MeView>, ApiError> {
    // `Caller`'s extractor resolves only from the session cookie, so `caller.id` is always a real
    // user row here — but a deleted account mid-session is still reachable, and that is a stale
    // session rather than a server fault.
    let user = state
        .stores()
        .user()
        .get_by_id(caller.id)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound("the signed-in account no longer exists".into()))?;

    Ok(Json(MeView {
        id: user.id,
        email: user.email,
        kind: kind_str(caller.kind),
    }))
}
