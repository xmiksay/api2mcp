//! Caller identity: who is making a request, in whichever of the three ways api2mcp can
//! resolve one ([`CallerKind`]).
//!
//! **Authorization is binary, not role-based.** A [`Caller`] resolved from a session cookie
//! (`CallerKind::Session`) or the CLI (`CallerKind::Cli`) can read and write every definition,
//! full stop — there is no admin/non-admin distinction among users any more. A service token
//! (`CallerKind::ServiceToken`) can only ever call tools over `/mcp`: [`Caller`]'s own
//! `FromRequestParts` impl below resolves *exclusively* from the session cookie and never even
//! inspects the `Authorization` header, so a bearer service token cannot construct a `Caller`
//! at all — that's what makes "a service token never reaches `/api/*`" a structural property of
//! the router (see `server::api`'s module doc) rather than a per-route checklist item.
//!
//! **There is no scope any more.** A service token used to carry a `scopes` list distinguishing
//! `"mcp"` (call tools) from `"admin"` (token/user management); the admin/non-admin distinction
//! is gone (Decision 1), which left exactly one possible value — a list that can only ever hold
//! one value encodes nothing, so `service_tokens.scopes`, `ServiceTokenRecord::scopes` and
//! `Caller::has_scope` are all gone with it (`migration::m0001_init`). The rule `server::mcp`
//! enforces before serving a tool call is simply: a resolved, unrevoked, unexpired token may
//! call tools. [`SCOPE_MCP`] survives only because `server::oauth` (this crate's own OAuth 2.1
//! authorization server, a separate concern from anything in this module) still uses it as its
//! one supported/default OAuth *scope* string — an unrelated meaning of the word "scope" that
//! has nothing to do with `Caller`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::store::{ServiceTokenRecord, UserRecord};

use super::auth;

/// `server::oauth`'s one supported/default OAuth scope string — see this module's own doc for
/// why it lives here despite no longer describing anything about `Caller`.
pub const SCOPE_MCP: &str = "mcp";

/// How a [`Caller`] was resolved. Never affects *what* a caller may do beyond the
/// `ServiceToken`-is-mcp-only split described in this module's own doc — the audit log
/// (`runs.caller_kind`) wants that distinct from the caller id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    /// A server-rendered browser session (the `/login` cookie).
    Session,
    /// A minted, hashed-at-rest bearer token (`api2mcp token mint`).
    ServiceToken,
    /// The trusted local operator running the `api2mcp` binary directly. Never reachable
    /// from a network request — there is no transport that produces this variant.
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub kind: CallerKind,
    pub id: Uuid,
}

impl Caller {
    pub fn from_user(user: &UserRecord) -> Self {
        Caller {
            kind: CallerKind::Session,
            id: user.id,
        }
    }

    pub fn from_service_token(record: &ServiceTokenRecord) -> Self {
        Caller {
            kind: CallerKind::ServiceToken,
            id: record.owner_id,
        }
    }

    /// The trusted local operator. `id` is the nil UUID: a CLI invocation has no session or
    /// token row to point at. A process that can run `api2mcp` at all already has full access
    /// to the database and every credential env var, so gating it further at this layer would
    /// be theatre.
    pub fn cli() -> Self {
        Caller {
            kind: CallerKind::Cli,
            id: Uuid::nil(),
        }
    }
}

/// What an axum `State` must expose for [`Caller`] to extract itself from a request's
/// session cookie. A trait rather than a direct dependency on `server::state::AppState` so this
/// module doesn't need to know that type's full shape — `AppState` implements this in one line
/// (`server/state.rs`) and the extractor below works unchanged.
pub trait AuthContext {
    fn auth_db(&self) -> &DatabaseConnection;
}

impl<S> FromRequestParts<S> for Caller
where
    S: AuthContext + Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session_token = parts
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| auth::cookie_value(h, auth::SESSION_COOKIE_NAME));
        let Some(token) = session_token else {
            return Err((StatusCode::UNAUTHORIZED, "no session cookie"));
        };
        match auth::resolve_session(state.auth_db(), token).await {
            Ok(Some(caller)) => Ok(caller),
            Ok(None) => Err((StatusCode::UNAUTHORIZED, "invalid or expired session")),
            Err(e) => {
                tracing::error!(error = %e, "Caller extractor: session lookup failed");
                Err((StatusCode::INTERNAL_SERVER_ERROR, "session lookup failed"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_record() -> ServiceTokenRecord {
        ServiceTokenRecord {
            restricted: false,
            id: Uuid::new_v4(),
            token_prefix: "abcdefgh".into(),
            owner_id: Uuid::new_v4(),
            label: "test".into(),
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
            created_at: chrono::Utc::now(),
            endpoints: Default::default(),
        }
    }

    #[test]
    fn cli_caller_is_its_own_kind_with_the_nil_id() {
        let caller = Caller::cli();
        assert_eq!(caller.kind, CallerKind::Cli);
        assert_eq!(caller.id, Uuid::nil());
    }

    #[test]
    fn from_user_produces_a_session_caller() {
        let user = crate::store::UserRecord {
            id: Uuid::new_v4(),
            email: "u@example.com".into(),
            has_password: true,
            oidc_issuer: None,
            oidc_subject: None,
            created_at: chrono::Utc::now(),
        };
        let caller = Caller::from_user(&user);
        assert_eq!(caller.kind, CallerKind::Session);
        assert_eq!(caller.id, user.id);
    }

    #[test]
    fn from_service_token_carries_the_owner_id_as_caller_id() {
        let record = token_record();
        let caller = Caller::from_service_token(&record);
        assert_eq!(caller.kind, CallerKind::ServiceToken);
        assert_eq!(caller.id, record.owner_id);
    }
}
