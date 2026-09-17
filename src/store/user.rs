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
    /// stay `NULL` too — nothing to link to yet. The row has no working login until
    /// [`Self::find_or_create_by_oidc`] *claims* it (binds `(issuer, subject)` in one atomic
    /// step) on the first OIDC sign-in with a **verified** email matching this row exactly —
    /// see that method's own doc for why matching on email is safe here specifically, unlike
    /// for an *already-linked* row. Until claimed, `Self::verify_password` correctly refuses
    /// this row (`password_hash` is `NULL`).
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

    /// Resolves a caller's OIDC identity to a [`UserRecord`]. Three outcomes, tried in order:
    ///
    /// 1. **A row already linked to `(issuer, subject)`.** Matched on that pair only — **never**
    ///    on `email`, which a provider is free to let a user change; matching a *linked* account
    ///    on email would let a changed email hijack it. The stored email is refreshed on every
    ///    sign-in, but never drives this lookup.
    /// 2. **No linked row, but `email_verified` is `true` and an unlinked row has this exact
    ///    email** — a pending [`Self::create_pending_oidc`] invite, or a password account from
    ///    [`Self::create`]. That row is *claimed*: `(issuer, subject)` is bound to it via one
    ///    atomic `UPDATE ... WHERE id = ? AND oidc_issuer IS NULL`, so two concurrent sign-ins
    ///    can never both believe they claimed it — the loser falls back to re-checking outcome 1.
    ///
    ///    Deliberately **not** the account-takeover shape outcome 1 warns about: there, email is
    ///    mutable data attached to an identity that *already exists*, so trusting it lets
    ///    someone who can set their email to yours assume your identity. Here the target row has
    ///    **no identity bound to it at all** — nothing to take over; it's either a pending invite
    ///    an operator deliberately created, or a password account whose owner is proving control
    ///    of the address via a real sign-in. `email_verified` is mandatory: an unverified email
    ///    is self-asserted, and claiming on it would be exactly the hole this avoids. A row
    ///    linked to a *different* `(issuer, subject)` is never a candidate here at all —
    ///    excluded by construction (`oidc_issuer IS NULL`).
    /// 3. **Otherwise, a new row is created and linked.** If the email already belongs to
    ///    someone (unverified, or linked to a different identity), that row is left untouched
    ///    and this returns [`StoreError::Conflict`] — `ux_users_email` is global, so two rows
    ///    can never actually share an address.
    pub async fn find_or_create_by_oidc(
        &self,
        issuer: &str,
        subject: &str,
        email: &str,
        email_verified: bool,
    ) -> Result<UserRecord, StoreError> {
        if let Some(user) = self.refresh_linked(issuer, subject, email).await? {
            return Ok(user);
        }

        if email_verified && let Some(candidate) = self.get_unlinked_by_email(email).await? {
            if self
                .claim_unlinked(candidate.id, issuer, subject, email)
                .await?
            {
                return self.get_by_id(candidate.id).await?.ok_or_else(|| {
                    StoreError::Internal("user disappeared immediately after being claimed".into())
                });
            }
            // Lost the race — a concurrent sign-in claimed this row first. One deployment has
            // one issuer, so that's normally the *same* identity signing in twice at once;
            // re-checking outcome 1 picks up the winner's row.
            if let Some(user) = self.refresh_linked(issuer, subject, email).await? {
                return Ok(user);
            }
        }

        // Outcome 3's typed `Conflict`, not a raw `ux_users_email` violation: catches both "no
        // candidate to claim" and "claim refused" (unverified, or linked to a different
        // identity) landing on an email someone already owns.
        if self.get_by_email(email).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "a user with email {email:?} already exists and cannot be claimed for this \
                 sign-in (its email is unverified, or the existing account is linked to a \
                 different identity)"
            )));
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

    /// Outcome 1 of [`Self::find_or_create_by_oidc`], split out so the claim path can retry it
    /// after losing a race without duplicating the email-refresh logic.
    async fn refresh_linked(
        &self,
        issuer: &str,
        subject: &str,
        email: &str,
    ) -> Result<Option<UserRecord>, StoreError> {
        let Some(existing) = self.get_by_oidc_identity(issuer, subject).await? else {
            return Ok(None);
        };
        if existing.email != email {
            users::Entity::update_many()
                .col_expr(users::Column::Email, Expr::value(email.to_owned()))
                .filter(users::Column::Id.eq(existing.id))
                .exec(&self.db)
                .await
                .map_err(db_err("user::find_or_create_by_oidc(refresh_email)"))?;
        }
        self.get_by_id(existing.id)
            .await?
            .ok_or_else(|| StoreError::Internal("user disappeared after email refresh".into()))
            .map(Some)
    }

    /// A row with this email and no OIDC identity bound at all — a claim candidate for
    /// [`Self::find_or_create_by_oidc`]. `ck_users_oidc_pair` guarantees `oidc_subject` is
    /// `NULL` wherever `oidc_issuer` is, so filtering on the one column is enough.
    async fn get_unlinked_by_email(&self, email: &str) -> Result<Option<UserRecord>, StoreError> {
        let row = users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .filter(users::Column::OidcIssuer.is_null())
            .one(&self.db)
            .await
            .map_err(db_err("user::get_unlinked_by_email"))?;
        Ok(row.map(to_model))
    }

    /// Binds `(issuer, subject)` to `id` iff still unlinked, in one statement — the
    /// `WHERE ... AND oidc_issuer IS NULL` is what makes this race-safe: at most one concurrent
    /// `UPDATE` can ever match the row. Returns whether *this* call was the one that won.
    async fn claim_unlinked(
        &self,
        id: Uuid,
        issuer: &str,
        subject: &str,
        email: &str,
    ) -> Result<bool, StoreError> {
        let res = users::Entity::update_many()
            .col_expr(users::Column::OidcIssuer, Expr::value(issuer.to_owned()))
            .col_expr(users::Column::OidcSubject, Expr::value(subject.to_owned()))
            .col_expr(users::Column::Email, Expr::value(email.to_owned()))
            .filter(users::Column::Id.eq(id))
            .filter(users::Column::OidcIssuer.is_null())
            .exec(&self.db)
            .await
            .map_err(db_err("user::claim_unlinked"))?;
        Ok(res.rows_affected == 1)
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
