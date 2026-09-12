//! The read-write admin JSON API (chunk C14): CRUD over every definition aggregate, plus the
//! read-only surfaces (health, `me`, runs, a resolved endpoint's plan) and the two test-run
//! routes that execute a definition for real and show its raw and projected output side by side.
//!
//! **Every route here is session-only, and any session may use it.** There is no admin/non-admin
//! distinction among users (Decision 1) — a valid [`Caller`] can read and write every definition,
//! full stop — so a route in this module needs no per-caller authorization check at all; it only
//! needs *a* `Caller` to have been extracted, which [`Caller`]'s own `FromRequestParts` impl
//! (`server::identity`) already guarantees resolves *only* from the session cookie. It never even
//! inspects the `Authorization` header, so a bearer service token cannot construct a `Caller` at
//! all, regardless of its scopes. That is what makes "a service token never reaches a route in
//! this module" (and, in particular, never reaches `POST /api/auth_providers` — I5) a structural
//! property of this router rather than a per-route checklist item: there is no code path from a
//! bearer token to a passing extraction here. MCP credentials keep authorizing tool *calls*
//! through `server::mcp`, entirely separately. Handlers still take `Caller` as an extractor
//! (`_caller` where the value itself is unused) purely to force that extraction to run.
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
mod tokens;
mod validate_write;

use axum::Router;
use axum::routing::get;

use super::error::ApiError;
use super::identity::Caller;
use super::state::AppState;

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
        .merge(tokens::router())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::identity::CallerKind;
    use uuid::Uuid;

    /// Pins the structural claim this module's doc makes: a `Caller` built directly (as the
    /// `ServiceToken` variant, bypassing the cookie-only extractor) is still a perfectly usable
    /// `Caller` value — the actual guarantee lives in the extractor never producing one from a
    /// bearer token in the first place, not in any per-route check on the value once extracted.
    #[test]
    fn a_caller_of_any_kind_is_a_valid_value_once_constructed() {
        let caller = Caller {
            kind: CallerKind::ServiceToken,
            id: Uuid::new_v4(),
        };
        assert_eq!(caller.kind, CallerKind::ServiceToken);
    }
}
