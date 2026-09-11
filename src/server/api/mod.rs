//! The read-write admin JSON API (chunk C14): CRUD over every definition aggregate, plus the
//! read-only surfaces (health, `me`, runs, a resolved endpoint's plan) and the two test-run
//! routes that execute a definition for real and show its raw and projected output side by side.
//!
//! **Every route here is admin-session only.** Each handler takes [`Caller`] as an extractor and
//! calls [`require_admin`] first. [`Caller`]'s own `FromRequestParts` impl (`server::identity`)
//! resolves *only* from the session cookie — it never even inspects the `Authorization` header —
//! so a bearer service token cannot construct a `Caller` at all, regardless of its scopes. That
//! is what makes "a service token never reaches a write route" (and, in particular, never reaches
//! `POST /api/auth_providers` — I5) a structural property of this router rather than a per-route
//! checklist item: there is no code path from a bearer token to a passing extraction here. MCP
//! credentials keep authorizing tool *calls* through `server::mcp`, entirely separately.
//!
//! Module layout: [`dto`] (wire shapes, reusing `pack::Pack*` types), [`convert`]/
//! [`convert_items`] (`model::` <-> those shapes), [`validate_write`] (the pre-write
//! `pack::validate` gate), [`test_run`] (shared plumbing for the two test-run routes), then one
//! file per resource.

mod api_calls;
mod auth_providers;
mod convert;
mod convert_items;
mod dto;
mod endpoints;
mod health;
mod me;
mod runs;
mod scripts;
mod services;
mod tags;
mod test_run;
mod validate_write;

use axum::Router;
use axum::routing::get;

use super::error::ApiError;
use super::identity::{Caller, assert_admin};
use super::state::AppState;

/// The one shared gate every handler in this module calls before doing anything user-visible —
/// see this module's own doc for why the extractor upstream of this call already does the hard
/// part (excluding a service token structurally) and this is only the human-vs-non-admin-human
/// check on top of that.
fn require_admin(caller: &Caller) -> Result<(), ApiError> {
    assert_admin(caller).map_err(|(_, message)| ApiError::Forbidden(message.to_owned()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health::health))
        .route("/me", get(me::me))
        .merge(services::router())
        .merge(auth_providers::router())
        .merge(api_calls::router())
        .merge(scripts::router())
        .merge(endpoints::router())
        .merge(tags::router())
        .merge(runs::router())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::identity::CallerKind;
    use uuid::Uuid;

    #[test]
    fn require_admin_rejects_a_non_admin_caller() {
        let caller = Caller {
            kind: CallerKind::Session,
            id: Uuid::new_v4(),
            is_admin: false,
            scopes: Vec::new(),
        };
        assert!(matches!(
            require_admin(&caller),
            Err(ApiError::Forbidden(_))
        ));
    }

    #[test]
    fn require_admin_accepts_an_admin_caller() {
        assert!(require_admin(&Caller::cli()).is_ok());
    }
}
