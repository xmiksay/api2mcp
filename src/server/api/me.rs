//! `GET /api/me` — who the caller is. Trivial, but it is what the admin SPA uses to confirm a
//! session is live and render the signed-in identity, so it earns its own tiny route.

use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::server::identity::CallerKind;

use super::{ApiError, Caller};

#[derive(Debug, Serialize)]
pub struct MeView {
    pub id: Uuid,
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

pub async fn me(caller: Caller) -> Result<Json<MeView>, ApiError> {
    Ok(Json(MeView {
        id: caller.id,
        kind: kind_str(caller.kind),
    }))
}
