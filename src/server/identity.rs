//! Caller identity: who is making a request, in whichever of the three ways api2mcp can
//! resolve one ([`CallerKind`]), plus [`assert_admin`] — the one shared gate every
//! admin-only surface calls before doing anything user-visible.
//!
//! **Service-token scopes hold exactly two values this iteration:** [`SCOPE_MCP`] (call
//! tools) and [`SCOPE_ADMIN`] (everything else — token/user management now, the read-only
//! admin API in a later chunk). Resist adding a third axis here: chess-base grew
//! `read_only` and `global_only` on top of its scope column and pays for the extra branch
//! in every dispatch check ever since. Two flat values keep [`Caller::has_scope`] — and
//! every caller of it — a one-line lookup.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::store::{ServiceTokenRecord, UserRecord};

use super::auth;

pub const SCOPE_MCP: &str = "mcp";
pub const SCOPE_ADMIN: &str = "admin";

/// How a [`Caller`] was resolved. Never affects *what* a caller may do on its own — that's
/// `is_admin`/`scopes` — only where the identity came from, which the audit log (chunk C7's
/// `runs.caller_kind`) wants to keep distinct from the caller id.
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
    pub is_admin: bool,
    /// Empty for `Session`/`Cli` — those two are always fully trusted, so there is no list
    /// to check for them (see [`Caller::has_scope`]). Populated only for `ServiceToken`.
    pub scopes: Vec<String>,
}

impl Caller {
    pub fn from_user(user: &UserRecord) -> Self {
        Caller {
            kind: CallerKind::Session,
            id: user.id,
            is_admin: user.is_admin,
            scopes: Vec::new(),
        }
    }

    pub fn from_service_token(record: &ServiceTokenRecord) -> Self {
        let is_admin = record.scopes.iter().any(|s| s == SCOPE_ADMIN);
        Caller {
            kind: CallerKind::ServiceToken,
            id: record.owner_id,
            is_admin,
            scopes: record.scopes.clone(),
        }
    }

    /// The trusted local operator. Always admin, in every scope — a process that can run
    /// `api2mcp` at all already has full access to the database and every credential env
    /// var, so gating it further at this layer would be theatre. `id` is the nil UUID: a
    /// CLI invocation has no session or token row to point at.
    pub fn cli() -> Self {
        Caller {
            kind: CallerKind::Cli,
            id: Uuid::nil(),
            is_admin: true,
            scopes: vec![SCOPE_MCP.to_owned(), SCOPE_ADMIN.to_owned()],
        }
    }

    /// `Session` and `Cli` callers satisfy any scope — they have no scope list to fall
    /// short of. Only `ServiceToken` actually checks membership.
    pub fn has_scope(&self, scope: &str) -> bool {
        match self.kind {
            CallerKind::Session | CallerKind::Cli => true,
            CallerKind::ServiceToken => self.scopes.iter().any(|s| s == scope),
        }
    }
}

/// What an axum `State` must expose for [`Caller`] to extract itself from a request's
/// session cookie. A trait rather than a direct dependency on `server::state::AppState`:
/// that type is chunk C11's, and does not exist yet when this module is written. C11
/// implements this for `AppState` in one line and the extractor below works unchanged.
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

/// Gate for anything that requires an admin caller. Every [`Caller`] variant already
/// carries the answer (`is_admin`); this is just the one shared place that turns "no" into
/// the response every admin-only surface should give, so that response can't drift between
/// call sites.
pub fn assert_admin(caller: &Caller) -> Result<(), (StatusCode, &'static str)> {
    if caller.is_admin {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "admin access required"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_record(scopes: &[&str]) -> ServiceTokenRecord {
        ServiceTokenRecord {
            id: Uuid::new_v4(),
            token_prefix: "abcdefgh".into(),
            owner_id: Uuid::new_v4(),
            label: "test".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn cli_caller_is_always_admin_and_every_scope() {
        let caller = Caller::cli();
        assert_eq!(caller.kind, CallerKind::Cli);
        assert!(caller.is_admin);
        assert!(caller.has_scope(SCOPE_MCP));
        assert!(caller.has_scope(SCOPE_ADMIN));
        assert!(caller.has_scope("anything"));
    }

    #[test]
    fn service_token_admin_scope_implies_is_admin() {
        let caller = Caller::from_service_token(&token_record(&["admin"]));
        assert!(caller.is_admin);
        assert!(caller.has_scope(SCOPE_ADMIN));
        assert!(!caller.has_scope(SCOPE_MCP));
    }

    #[test]
    fn service_token_mcp_only_scope_is_not_admin() {
        let caller = Caller::from_service_token(&token_record(&["mcp"]));
        assert!(!caller.is_admin);
        assert!(caller.has_scope(SCOPE_MCP));
        assert!(!caller.has_scope(SCOPE_ADMIN));
    }

    #[test]
    fn assert_admin_rejects_a_non_admin_caller() {
        let caller = Caller::from_service_token(&token_record(&["mcp"]));
        let err = assert_admin(&caller).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn assert_admin_accepts_an_admin_caller() {
        assert!(assert_admin(&Caller::cli()).is_ok());
    }
}
