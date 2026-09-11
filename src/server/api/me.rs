//! `GET /api/me` — who the caller is. Trivial, but it is what an admin SPA uses to confirm a
//! session is live and render the signed-in identity, so it earns its own tiny route.

use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::server::identity::CallerKind;

use super::{ApiError, Caller, require_admin};

#[derive(Debug, Serialize)]
pub struct MeView {
    pub id: Uuid,
    pub kind: &'static str,
    pub is_admin: bool,
}

fn kind_str(kind: CallerKind) -> &'static str {
    match kind {
        CallerKind::Session => "session",
        CallerKind::ServiceToken => "service_token",
        CallerKind::Cli => "cli",
    }
}

pub async fn me(caller: Caller) -> Result<Json<MeView>, ApiError> {
    require_admin(&caller)?;
    Ok(Json(MeView {
        id: caller.id,
        kind: kind_str(caller.kind),
        is_admin: caller.is_admin,
    }))
}
