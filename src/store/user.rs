//! `users` façade. There is no admin/non-admin distinction — see `entity::users`'s own doc —
//! so this store's job is purely identity: create/find a user by password or by OIDC identity,
//! verify a password, list, delete.
//!
//! Local passwords are hashed argon2id (never sha256 — a human-chosen password has far less
//! entropy than a minted token, so it needs a slow KDF; see [`super::service_token`] for the
//! opposite call).
//!
//! There is no `model::User` — identity isn't one of the tool-definition aggregates the
//! `model` module models, so this store defines its own plain, scalar-only [`UserRecord`]/
//! [`NewUser`] return types rather than reaching for a type that doesn't exist. Neither
//! wraps an `entity::Model`, so the layering invariant still holds.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    /// `true` for a local-password account. `false` covers both a linked OIDC account
    /// (`oidc_issuer`/`oidc_subject` set) and a pending, not-yet-linked one created by
    /// `api2mcp user add --oidc-only` (both `None`) — see [`UserStore::create_pending_oidc`].
    pub has_password: bool,
    pub oidc_issuer: Option<String>,
    pub oidc_subject: Option<String>,
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

    /// Creates a local-password user. Errors with [`StoreError::Conflict`] if the email is
    /// already taken — by either kind of account, since `ux_users_email` is a global unique
    /// index regardless of login method.
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
            password_hash: Set(Some(hash)),
            oidc_issuer: Set(None),
            oidc_subject: Set(None),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("user::create"))?;
        self.get_by_id(id)
            .await?
            .ok_or_else(|| StoreError::Internal("user disappeared immediately after insert".into()))
    }

    /// Pre-provisions an account with no password (`api2mcp user add --oidc-only`), for an
    /// operator onboarding someone who will sign in externally. `oidc_issuer`/`oidc_subject`
    /// stay `NULL` here too — this does **not** link the account to any identity, because
    /// there is nothing to link to yet: linking a pre-provisioned row to the OIDC identity that
    /// later signs in with a matching email is real work (matching on email is exactly what
    /// `find_or_create_by_oidc`'s own doc warns is an account-takeover hole, so a same-email
    /// claim step would need to prove that risk is acceptable for a not-yet-linked row
    /// specifically) — left for a follow-up chunk. Until then, a pending row created here has
    /// no working login at all; `Self::verify_password` correctly refuses it (`password_hash`
    /// is `NULL`), and `find_or_create_by_oidc` never notices it exists (it doesn't search by
    /// email).
    pub async fn create_pending_oidc(&self, email: &str) -> Result<UserRecord, StoreError> {
        if self.get_by_email(email).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "a user with email {email:?} already exists"
            )));
        }
        let id = Uuid::new_v4();
        users::ActiveModel {
            id: Set(id),
            email: Set(email.to_owned()),
            password_hash: Set(None),
            oidc_issuer: Set(None),
            oidc_subject: Set(None),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("user::create_pending_oidc"))?;
        self.get_by_id(id)
            .await?
            .ok_or_else(|| StoreError::Internal("user disappeared immediately after insert".into()))
    }

    /// Resolves a caller's OIDC identity to a [`UserRecord`], creating one on first sign-in.
    /// Matches on `(issuer, subject)` only — **never** on `email`, which a provider is free to
    /// let a user change; matching on it would let a changed email hijack another account. The
    /// stored email is refreshed on every sign-in so it stays current for display, but it is
    /// never part of the lookup.
    pub async fn find_or_create_by_oidc(
        &self,
        issuer: &str,
        subject: &str,
        email: &str,
    ) -> Result<UserRecord, StoreError> {
        if let Some(existing) = self.get_by_oidc_identity(issuer, subject).await? {
            if existing.email != email {
                users::Entity::update_many()
                    .col_expr(users::Column::Email, Expr::value(email.to_owned()))
                    .filter(users::Column::Id.eq(existing.id))
                    .exec(&self.db)
                    .await
                    .map_err(db_err("user::find_or_create_by_oidc(refresh_email)"))?;
            }
            return self.get_by_id(existing.id).await?.ok_or_else(|| {
                StoreError::Internal("user disappeared after email refresh".into())
            });
        }

        let id = Uuid::new_v4();
        users::ActiveModel {
            id: Set(id),
            email: Set(email.to_owned()),
            password_hash: Set(None),
            oidc_issuer: Set(Some(issuer.to_owned())),
            oidc_subject: Set(Some(subject.to_owned())),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("user::find_or_create_by_oidc(create)"))?;
        self.get_by_id(id)
            .await?
            .ok_or_else(|| StoreError::Internal("user disappeared immediately after insert".into()))
    }

    pub async fn get_by_oidc_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<UserRecord>, StoreError> {
        let row = users::Entity::find()
            .filter(users::Column::OidcIssuer.eq(issuer))
            .filter(users::Column::OidcSubject.eq(subject))
            .one(&self.db)
            .await
            .map_err(db_err("user::get_by_oidc_identity"))?;
        Ok(row.map(to_model))
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

    /// Replaces `email`'s password hash. Returns [`StoreError::NotFound`] when no such user
    /// exists — unlike [`Self::verify_password`], which deliberately cannot distinguish that
    /// case, an administrator changing a password needs to know the account was not found.
    pub async fn set_password(&self, email: &str, password: &str) -> Result<(), StoreError> {
        let hash = hash_password(password)?;
        let res = users::Entity::update_many()
            .col_expr(users::Column::PasswordHash, Expr::value(Some(hash)))
            .filter(users::Column::Email.eq(email))
            .exec(&self.db)
            .await
            .map_err(db_err("user::set_password"))?;
        if res.rows_affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Verifies `password` against `email`'s stored hash. Returns `Ok(None)` uniformly for
    /// "no such user", "wrong password" and "this account has no password (OIDC-only)" alike —
    /// a caller must not be able to distinguish those from this method's return shape alone
    /// (constant-time comparison and mitigating response-timing differences is a `server`-layer
    /// concern, out of scope here).
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
        let Some(stored_hash) = &row.password_hash else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(stored_hash).map_err(|e| {
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
        has_password: row.password_hash.is_some(),
        oidc_issuer: row.oidc_issuer,
        oidc_subject: row.oidc_subject,
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
