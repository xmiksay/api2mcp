//! `oauth_tokens` — hashed access/refresh pairs, split out of `oauth.rs` to keep both files
//! under the line cap; these methods extend [`super::oauth::OauthStore`] rather than define
//! a separate façade, since a token is never meaningful without its issuing client/consent.
//!
//! `family_id` implements RFC 9700 §6.1's rotation-and-reuse-detection recommendation:
//! [`OauthStore::rotate_refresh_token`] revokes the presented row and issues a fresh pair in
//! the same family; if the *same* (already-revoked) refresh token is ever presented again —
//! proof someone is replaying a stolen token — [`OauthStore::revoke_family`] kills every
//! token descended from that authorization, not just the one being reused.

use std::time::Duration;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use uuid::Uuid;

use crate::entity::oauth_tokens;

use super::oauth::OauthStore;
use super::service_token::generate_plaintext;
use super::{StoreError, db_err, sha256_hex};

/// A freshly issued (or rotated) pair. Both token strings exist only here — they are never
/// stored, only their sha256 hashes are.
#[derive(Debug, Clone)]
pub struct IssuedOauthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub family_id: Uuid,
}

/// Bundles a token's identity fields — kept separate from the TTLs and the `family_id` that
/// vary by call site, so [`OauthStore::issue_token`]/`issue_in_family` stay under clippy's
/// argument-count limit.
#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthToken {
    pub id: i64,
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub scope: Option<String>,
    pub resource: Option<String>,
    pub revoked: bool,
    pub access_expires_at: chrono::DateTime<Utc>,
    pub refresh_expires_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
}

impl OauthStore {
    pub async fn issue_token(
        &self,
        grant: TokenGrant,
        access_ttl: Duration,
        refresh_ttl: Option<Duration>,
    ) -> Result<IssuedOauthToken, StoreError> {
        self.issue_in_family(grant, Uuid::new_v4(), access_ttl, refresh_ttl)
            .await
    }

    /// `Ok(None)` for "no such refresh token" and "expired" alike. A **revoked** token is
    /// different: it means this exact token was already rotated away once, so presenting it
    /// again is reuse — the whole family is revoked in response (see the module doc) before
    /// returning `Ok(None)`.
    pub async fn rotate_refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<Option<IssuedOauthToken>, StoreError> {
        let hash = sha256_hex(refresh_token.as_bytes());
        let Some(row) = self.find_by_refresh_hash(&hash).await? else {
            return Ok(None);
        };
        if row.revoked {
            self.revoke_family(row.family_id).await?;
            return Ok(None);
        }
        if let Some(exp) = row.refresh_expires_at
            && exp.with_timezone(&Utc) <= Utc::now()
        {
            return Ok(None);
        }

        let created_at = row.created_at.with_timezone(&Utc);
        let access_ttl = (row.access_expires_at.with_timezone(&Utc) - created_at)
            .to_std()
            .map_err(|e| StoreError::Internal(format!("oauth_tokens: negative access TTL: {e}")))?;
        let refresh_ttl = row
            .refresh_expires_at
            .map(|exp| (exp.with_timezone(&Utc) - created_at).to_std())
            .transpose()
            .map_err(|e| {
                StoreError::Internal(format!("oauth_tokens: negative refresh TTL: {e}"))
            })?;

        let mut active: oauth_tokens::ActiveModel = row.clone().into();
        active.revoked = Set(true);
        active
            .update(&self.db)
            .await
            .map_err(db_err("oauth::rotate_refresh_token"))?;

        let grant = TokenGrant {
            client_id: row.client_id,
            user_id: row.user_id,
            scope: row.scope,
            resource: row.resource,
        };
        let issued = self
            .issue_in_family(grant, row.family_id, access_ttl, refresh_ttl)
            .await?;
        Ok(Some(issued))
    }

    /// Revokes every token descended from `family_id` — the whole-family response to
    /// detected refresh-token reuse.
    pub async fn revoke_family(&self, family_id: Uuid) -> Result<(), StoreError> {
        let rows = oauth_tokens::Entity::find()
            .filter(oauth_tokens::Column::FamilyId.eq(family_id))
            .all(&self.db)
            .await
            .map_err(db_err("oauth::revoke_family"))?;
        for row in rows {
            if row.revoked {
                continue;
            }
            let mut active: oauth_tokens::ActiveModel = row.into();
            active.revoked = Set(true);
            active
                .update(&self.db)
                .await
                .map_err(db_err("oauth::revoke_family"))?;
        }
        Ok(())
    }

    /// The `created_at` of the oldest row sharing `family_id` — the family's original
    /// issuance time. [`rotate_refresh_token`](Self::rotate_refresh_token) only ever carries
    /// each row's own TTL *duration* forward, never an absolute deadline, so an absolute
    /// family lifetime (chunk C12's `server::oauth::refresh`) has nothing else to check
    /// against. `None` only for an unknown `family_id` — never for one that has just rotated,
    /// since the row inserted by that rotation is itself a family member.
    pub async fn family_started_at(
        &self,
        family_id: Uuid,
    ) -> Result<Option<chrono::DateTime<Utc>>, StoreError> {
        let oldest = oauth_tokens::Entity::find()
            .filter(oauth_tokens::Column::FamilyId.eq(family_id))
            .order_by_asc(oauth_tokens::Column::CreatedAt)
            .one(&self.db)
            .await
            .map_err(db_err("oauth::family_started_at"))?;
        Ok(oldest.map(|r| r.created_at.with_timezone(&Utc)))
    }

    /// `Ok(None)` for "no such token", "revoked" and "expired" alike.
    pub async fn resolve_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<OauthToken>, StoreError> {
        let hash = sha256_hex(access_token.as_bytes());
        let Some(row) = oauth_tokens::Entity::find()
            .filter(oauth_tokens::Column::AccessTokenHash.eq(hash))
            .one(&self.db)
            .await
            .map_err(db_err("oauth::resolve_access_token"))?
        else {
            return Ok(None);
        };
        if row.revoked || row.access_expires_at.with_timezone(&Utc) <= Utc::now() {
            return Ok(None);
        }
        Ok(Some(to_model(row)))
    }

    async fn issue_in_family(
        &self,
        grant: TokenGrant,
        family_id: Uuid,
        access_ttl: Duration,
        refresh_ttl: Option<Duration>,
    ) -> Result<IssuedOauthToken, StoreError> {
        let now = Utc::now();
        let access_expires_at = now
            + chrono::Duration::from_std(access_ttl).map_err(|e| {
                StoreError::Internal(format!("oauth_tokens: access_ttl out of range: {e}"))
            })?;
        let refresh_expires_at = refresh_ttl
            .map(|ttl| {
                chrono::Duration::from_std(ttl)
                    .map(|d| now + d)
                    .map_err(|e| {
                        StoreError::Internal(format!("oauth_tokens: refresh_ttl out of range: {e}"))
                    })
            })
            .transpose()?;

        let access_token = generate_plaintext();
        let refresh_token = refresh_ttl.map(|_| generate_plaintext());

        oauth_tokens::ActiveModel {
            access_token_hash: Set(sha256_hex(access_token.as_bytes())),
            refresh_token_hash: Set(refresh_token.as_deref().map(|t| sha256_hex(t.as_bytes()))),
            client_id: Set(grant.client_id),
            user_id: Set(grant.user_id),
            family_id: Set(family_id),
            scope: Set(grant.scope),
            resource: Set(grant.resource),
            revoked: Set(false),
            access_expires_at: Set(access_expires_at.into()),
            refresh_expires_at: Set(refresh_expires_at.map(Into::into)),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("oauth::issue_in_family"))?;

        Ok(IssuedOauthToken {
            access_token,
            refresh_token,
            family_id,
        })
    }

    async fn find_by_refresh_hash(
        &self,
        hash: &str,
    ) -> Result<Option<oauth_tokens::Model>, StoreError> {
        oauth_tokens::Entity::find()
            .filter(oauth_tokens::Column::RefreshTokenHash.eq(hash))
            .one(&self.db)
            .await
            .map_err(db_err("oauth::rotate_refresh_token"))
    }
}

fn to_model(row: oauth_tokens::Model) -> OauthToken {
    OauthToken {
        id: row.id,
        client_id: row.client_id,
        user_id: row.user_id,
        family_id: row.family_id,
        scope: row.scope,
        resource: row.resource,
        revoked: row.revoked,
        access_expires_at: row.access_expires_at.with_timezone(&Utc),
        refresh_expires_at: row.refresh_expires_at.map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.with_timezone(&Utc),
    }
}
