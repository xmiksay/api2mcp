//! Browser login sessions.
//!
//! A session token is a bearer credential exactly like a service token, so it is stored the
//! same way: sha256 of the plaintext as the primary key, never the plaintext itself. A stolen
//! database dump therefore yields no usable session.

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entity::{sessions, users};

use super::{StoreError, db_err, sha256_hex};

/// The user behind a resolved session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionUser {
    pub user_id: Uuid,
    pub is_admin: bool,
}

#[derive(Clone)]
pub struct SessionStore {
    db: DatabaseConnection,
}

impl SessionStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Stores a session for `user_id`, keyed by the hash of `plaintext`.
    pub async fn create(
        &self,
        plaintext: &str,
        user_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sessions::ActiveModel {
            token_hash: Set(sha256_hex(plaintext.as_bytes())),
            user_id: Set(user_id),
            expires_at: Set(expires_at.into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("session::create"))?;
        Ok(())
    }

    /// Resolves a caller-presented cookie value. `Ok(None)` covers "no such session",
    /// "expired" and "the user has since been deleted" alike: a caller must not be able to
    /// tell those apart, for the same reason `ServiceTokenStore::resolve` does not.
    pub async fn resolve(&self, plaintext: &str) -> Result<Option<SessionUser>, StoreError> {
        let hash = sha256_hex(plaintext.as_bytes());
        let Some(row) = sessions::Entity::find_by_id(hash)
            .one(&self.db)
            .await
            .map_err(db_err("session::resolve"))?
        else {
            return Ok(None);
        };
        if row.expires_at.with_timezone(&Utc) <= Utc::now() {
            return Ok(None);
        }
        let Some(user) = users::Entity::find_by_id(row.user_id)
            .one(&self.db)
            .await
            .map_err(db_err("session::resolve_user"))?
        else {
            return Ok(None);
        };
        Ok(Some(SessionUser {
            user_id: user.id,
            is_admin: user.is_admin,
        }))
    }

    /// Logout. Deleting a session that is already gone is not an error — logout is idempotent,
    /// and a caller retrying it must not see a failure.
    pub async fn delete(&self, plaintext: &str) -> Result<(), StoreError> {
        sessions::Entity::delete_by_id(sha256_hex(plaintext.as_bytes()))
            .exec(&self.db)
            .await
            .map_err(db_err("session::delete"))?;
        Ok(())
    }

    /// Drops every session whose expiry has passed. Called by the retention task; sessions are
    /// otherwise only removed on logout, so without this the table grows without bound.
    pub async fn purge_expired(&self) -> Result<u64, StoreError> {
        let res = sessions::Entity::delete_many()
            .filter(sessions::Column::ExpiresAt.lt(Utc::now()))
            .exec(&self.db)
            .await
            .map_err(db_err("session::purge_expired"))?;
        Ok(res.rows_affected)
    }
}
