//! `refresh_token` grant — split out of `token.rs` for the 400-line cap, per the plan's own
//! named seam.
//!
//! [`crate::store::OauthStore::rotate_refresh_token`] already implements rotation and reuse
//! detection (RFC 9700 §6.1): presenting an already-rotated-away (`revoked`) refresh token
//! revokes its whole family before this function is ever called. What it does *not* do is
//! enforce an **absolute** family lifetime, because it only ever carries a token's own TTL
//! *duration* forward across a rotation — never an absolute deadline — so a family could
//! otherwise live forever by rotating just before each per-token expiry. [`ABSOLUTE_FAMILY_TTL`]
//! is that missing check, made here against
//! [`OauthStore::family_started_at`](crate::store::OauthStore::family_started_at) (the
//! family's original issuance time), which doesn't move as the pair rotates.

use axum::response::Response;
use chrono::{Duration, Utc};

use crate::server::state::AppState;

use super::shared::{OAuthError, token_response};
use super::token::{ACCESS_TOKEN_TTL_SECS, TokenRequest};

/// Hard ceiling on a refresh-token family's lifetime, regardless of how many times it has
/// rotated since the original authorization. This is what turns a stolen-and-replayed refresh
/// token into a bounded-time problem even if reuse detection is somehow never triggered.
pub const ABSOLUTE_FAMILY_TTL_DAYS: i64 = 30;

pub async fn grant(state: &AppState, req: TokenRequest) -> Result<Response, OAuthError> {
    let refresh_token = req
        .refresh_token
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "refresh_token is required"))?;

    let store = state.stores().oauth();
    let issued = store
        .rotate_refresh_token(refresh_token)
        .await?
        .ok_or_else(|| {
            OAuthError::bad(
                "invalid_grant",
                "unknown, reused, revoked, or expired refresh token",
            )
        })?;

    // The row `rotate_refresh_token` just inserted is itself a family member, so
    // `family_started_at` never returns `None` here in practice; falling back to "now" (i.e.
    // "not expired") rather than failing the request is the fail-open-on-our-own-bug choice —
    // a spurious `None` must not lock a legitimate client out.
    let started = store
        .family_started_at(issued.family_id)
        .await?
        .unwrap_or_else(Utc::now);
    if Utc::now() - started > Duration::days(ABSOLUTE_FAMILY_TTL_DAYS) {
        // The pair just minted above is already a member of this family, so revoking the
        // family here also kills it — the client gets nothing usable back.
        store.revoke_family(issued.family_id).await?;
        return Err(OAuthError::bad(
            "invalid_grant",
            "refresh token family exceeded its absolute lifetime",
        ));
    }

    // `IssuedOauthToken` doesn't carry `scope` (rotation reuses the original grant's scope
    // internally but has no reason to hand it back out) — RFC 6749 §5.1 makes `scope` optional
    // in a token response when it's unchanged from the original grant, so omitting it here is
    // spec-compliant, not a shortcut.
    Ok(token_response(
        &issued.access_token,
        issued.refresh_token.as_deref(),
        ACCESS_TOKEN_TTL_SECS as i64,
        None,
    ))
}
