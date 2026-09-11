//! MCP authentication and the session-cookie plumbing the browser login path
//! ([`super::login`]) builds on.
//!
//! A `/mcp` request carries `Authorization: Bearer <token>`. [`authenticate_mcp`] resolves
//! it against the service-token store (which already rejects a revoked or expired row —
//! see [`crate::store::ServiceTokenStore::resolve`]) and, on any failure, returns a
//! [`BearerChallenge`]: the caller (chunk C11's router) turns that into a `401` carrying
//! `WWW-Authenticate: Bearer resource_metadata="…"`, the RFC 9728 discovery hook an
//! OAuth-aware MCP client follows to find the authorization server. Chunk C12's OAuth
//! access-token branch slots in ahead of the service-token check below — same bearer, same
//! header, just a second store tried before giving up.
//!
//! **Two hashing schemes, deliberately not shared:** [`hash_token`] is sha256
//! ([`crate::store::sha256_hex`]) — right for a service token or session cookie, both
//! 256-bit CSPRNG output ([`new_token`]) with no human-guessable structure, so a fast hash
//! costs an attacker exactly as much as it costs us. Human-chosen *passwords* go through
//! `argon2id` instead ([`crate::store::user`]) precisely because they don't have that
//! entropy floor — a KDF is what makes an offline guess expensive when the input space is
//! small. Putting a KDF on every MCP call would tax every request for a property tokens
//! already have for free.
//!
//! **No `store::session` module exists yet** (only `store::user`, `store::service_token`
//! and `store::oauth` are implemented so far, and this chunk may not add to `src/store/`).
//! [`create_session`]/[`resolve_session`]/[`delete_session`] therefore talk to
//! `entity::sessions`/`entity::users` directly with `sea_orm`, which is a deliberate,
//! narrow exception to the crate's "sea_orm lives in exactly three modules" layering rule
//! (see `src/lib.rs`). A follow-up chunk should land a proper `SessionStore` façade and
//! fold this back in.

use axum::http::{HeaderMap, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use rand::RngCore;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::entity::{sessions, users};
use crate::store::ServiceTokenStore;

use super::identity::{Caller, CallerKind};

/// Name of the browser session cookie set by [`super::login::post_login`].
pub const SESSION_COOKIE_NAME: &str = "a2m_session";

/// A `401` bearer challenge: the `WWW-Authenticate` value pointing an MCP client at the
/// protected-resource metadata so it can discover OAuth. Plain data — building the actual
/// HTTP response is chunk C11's job (it owns `server/error.rs` and the router).
pub struct BearerChallenge {
    pub www_authenticate: String,
}

/// Resolve the caller behind an MCP request: bearer service token only, this chunk (OAuth
/// access tokens are chunk C12). No credential, or a credential that fails to resolve,
/// both come back as the same [`BearerChallenge`] — a caller must not be able to tell
/// "missing" from "invalid" from the response shape alone.
pub async fn authenticate_mcp(
    db: &DatabaseConnection,
    cfg: &Config,
    headers: &HeaderMap,
) -> std::result::Result<Caller, BearerChallenge> {
    let Some(token) = bearer_token(headers) else {
        return Err(challenge(cfg));
    };

    // OAuth access-token branch (C12) goes here, ahead of the service-token fallback.

    let store = ServiceTokenStore::new(db.clone());
    match store.resolve(token).await {
        Ok(Some(record)) => Ok(Caller::from_service_token(&record)),
        Ok(None) => Err(challenge(cfg)),
        Err(e) => {
            // Fail closed: a store error resolving the token is not evidence the token is
            // valid, so it must not be treated any more favourably than "not found".
            tracing::error!(error = %e, "authenticate_mcp: service token lookup failed");
            Err(challenge(cfg))
        }
    }
}

fn challenge(cfg: &Config) -> BearerChallenge {
    BearerChallenge {
        www_authenticate: format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
            cfg.base_url
        ),
    }
}

/// Extracts the token from `Authorization: Bearer <token>`, case-insensitively on the
/// scheme (RFC 6750 doesn't require clients to get the case right and several don't).
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Finds `name`'s value in a raw `Cookie` header (`"a=1; b=2"`). Used for both the session
/// cookie (browser login) and, once C11 wires it up, nowhere else — there is exactly one
/// cookie this crate sets or reads.
pub fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// A random, unguessable opaque token: 256 bits from the OS CSPRNG, base64url-encoded with
/// no padding. Used for both service tokens (via the CLI, `store::service_token`) and
/// session cookies (here) — the same "opaque, high-entropy, server-generated" shape either
/// way, just hashed into a different table.
pub fn new_token() -> String {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

/// Lowercase hex sha256 of `token` — delegates to [`crate::store::sha256_hex`] so a session
/// cookie and a service token are hashed identically at rest; see the module doc for why
/// sha256 (not argon2) is the right choice for both.
pub fn hash_token(token: &str) -> String {
    crate::store::sha256_hex(token.as_bytes())
}

/// Creates a session row for `user_id` and returns the plaintext cookie value — never
/// stored, only its hash is. `ttl` is `cfg.session_ttl`.
pub async fn create_session(
    db: &DatabaseConnection,
    user_id: Uuid,
    ttl: std::time::Duration,
) -> Result<String> {
    let plaintext = new_token();
    let ttl = chrono::Duration::from_std(ttl).context("session TTL out of range")?;
    let expires_at = Utc::now() + ttl;
    sessions::ActiveModel {
        token_hash: Set(hash_token(&plaintext)),
        user_id: Set(user_id),
        expires_at: Set(expires_at.into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .context("inserting session row")?;
    Ok(plaintext)
}

/// Resolves a caller-presented session cookie value to its [`Caller`]. `Ok(None)` for "no
/// such session" and "expired" alike, and also when the session's `user_id` no longer
/// resolves (a deleted user) — same "don't let a caller distinguish the failure modes"
/// reasoning as `ServiceTokenStore::resolve`.
pub async fn resolve_session(db: &DatabaseConnection, plaintext: &str) -> Result<Option<Caller>> {
    let hash = hash_token(plaintext);
    let Some(row) = sessions::Entity::find_by_id(hash)
        .one(db)
        .await
        .context("querying session")?
    else {
        return Ok(None);
    };
    if row.expires_at.with_timezone(&Utc) <= Utc::now() {
        return Ok(None);
    }
    let Some(user) = users::Entity::find_by_id(row.user_id)
        .one(db)
        .await
        .context("querying session user")?
    else {
        return Ok(None);
    };
    Ok(Some(Caller {
        kind: CallerKind::Session,
        id: user.id,
        is_admin: user.is_admin,
        scopes: Vec::new(),
    }))
}

/// Deletes a session row (logout). Deleting a row that doesn't exist (already expired,
/// already logged out elsewhere) is not an error — logout is idempotent.
pub async fn delete_session(db: &DatabaseConnection, plaintext: &str) -> Result<()> {
    let hash = hash_token(plaintext);
    sessions::Entity::delete_by_id(hash)
        .exec(db)
        .await
        .context("deleting session")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn cfg() -> Config {
        Config {
            database_url: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            base_url: "http://h:8080".into(),
            default_endpoint: "default".into(),
            admin_email: None,
            admin_password: None,
            run_retention_days: 30,
            allow_loopback_upstream: false,
            session_ttl: std::time::Duration::from_secs(3600),
            max_request_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn challenge_points_at_protected_resource_metadata() {
        let c = challenge(&cfg());
        assert_eq!(
            c.www_authenticate,
            r#"Bearer resource_metadata="http://h:8080/.well-known/oauth-protected-resource""#
        );
    }

    #[test]
    fn bearer_token_extracts_case_insensitively() {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc123"),
        );
        assert_eq!(bearer_token(&h), Some("abc123"));

        let mut h2 = HeaderMap::new();
        h2.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer xyz"),
        );
        assert_eq!(bearer_token(&h2), Some("xyz"));
    }

    #[test]
    fn bearer_token_rejects_other_schemes_and_missing_header() {
        let h = HeaderMap::new();
        assert_eq!(bearer_token(&h), None);

        let mut h2 = HeaderMap::new();
        h2.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc123"),
        );
        assert_eq!(bearer_token(&h2), None);
    }

    #[test]
    fn cookie_value_finds_a_named_cookie_among_several() {
        let raw = "a2m_session=tok123; other=val; third=x";
        assert_eq!(cookie_value(raw, "a2m_session"), Some("tok123"));
        assert_eq!(cookie_value(raw, "other"), Some("val"));
        assert_eq!(cookie_value(raw, "missing"), None);
    }

    #[test]
    fn new_token_is_256_bits_and_unique() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        // base64url, no padding, of 32 raw bytes: ceil(32 * 4 / 3) = 43 chars.
        assert_eq!(a.len(), 43);
    }

    #[test]
    fn hash_token_matches_the_store_sha256() {
        assert_eq!(hash_token("x"), crate::store::sha256_hex(b"x"));
    }
}
