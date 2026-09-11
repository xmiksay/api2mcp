//! `users` façade — admin accounts for the server-rendered login. Passwords are hashed
//! argon2id (never sha256 — a human-chosen password has far less entropy than a minted
//! token, so it needs a slow KDF; see [`super::service_token`] for the opposite call).
//!
//! There is no `model::User` — identity isn't one of the tool-definition aggregates the
//! `model` module models, so this store defines its own plain, scalar-only [`UserRecord`]/
//! [`NewUser`] return types rather than reaching for a type that doesn't exist. Neither
//! wraps an `entity::Model`, so the layering invariant still holds.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::entity::users;

use super::{StoreError, db_err};

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    /// Plaintext, hashed inside [`UserStore::create`]. Never stored or logged as-is.
    pub password: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct UserStore {
    db: DatabaseConnection,
}

impl UserStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn create(&self, new: NewUser) -> Result<UserRecord, StoreError> {
        if self.get_by_email(&new.email).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "a user with email {:?} already exists",
                new.email
            )));
        }
        let hash = hash_password(&new.password)?;
        let id = Uuid::new_v4();
        users::ActiveModel {
            id: Set(id),
            email: Set(new.email),
            password_hash: Set(hash),
            is_admin: Set(new.is_admin),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("user::create"))?;
        self.get_by_id(id)
            .await?
            .ok_or_else(|| StoreError::Internal("user disappeared immediately after insert".into()))
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<UserRecord>, StoreError> {
        let row = users::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("user::get_by_id"))?;
        Ok(row.map(to_model))
    }

    pub async fn get_by_email(&self, email: &str) -> Result<Option<UserRecord>, StoreError> {
        let row = users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .one(&self.db)
            .await
            .map_err(db_err("user::get_by_email"))?;
        Ok(row.map(to_model))
    }

    pub async fn list(&self) -> Result<Vec<UserRecord>, StoreError> {
        let rows = users::Entity::find()
            .order_by_asc(users::Column::Email)
            .all(&self.db)
            .await
            .map_err(db_err("user::list"))?;
        Ok(rows.into_iter().map(to_model).collect())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let res = users::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(db_err("user::delete"))?;
        if res.rows_affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Verifies `password` against `email`'s stored hash. Returns `Ok(None)` uniformly for
    /// both "no such user" and "wrong password" — a caller must not be able to distinguish
    /// the two from this method's return shape alone (constant-time comparison and
    /// mitigating response-timing differences is a `server`-layer concern, out of scope
    /// here).
    pub async fn verify_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<Option<UserRecord>, StoreError> {
        let Some(row) = users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .one(&self.db)
            .await
            .map_err(db_err("user::verify_password"))?
        else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&row.password_hash).map_err(|e| {
            StoreError::Internal(format!(
                "stored password hash for {email:?} is unparsable: {e}"
            ))
        })?;
        if Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
        {
            Ok(Some(to_model(row)))
        } else {
            Ok(None)
        }
    }
}

fn hash_password(password: &str) -> Result<String, StoreError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| StoreError::Internal(format!("hashing password: {e}")))
}

fn to_model(row: users::Model) -> UserRecord {
    UserRecord {
        id: row.id,
        email: row.email,
        is_admin: row.is_admin,
        created_at: row.created_at.with_timezone(&Utc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(
            Argon2::default()
                .verify_password(b"correct horse battery staple", &parsed)
                .is_ok()
        );
        assert!(
            Argon2::default()
                .verify_password(b"wrong password", &parsed)
                .is_err()
        );
    }
}
