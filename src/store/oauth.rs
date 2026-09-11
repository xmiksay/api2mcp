//! OAuth 2.1 AS façade, minus the token table (see [`super::oauth_tokens`], split out to
//! keep both files under the line cap): dynamically registered clients, single-use PKCE
//! codes, standing consents and the consent-request state the server-rendered consent
//! screen round-trips through `/oauth/authorize`.
//!
//! Like `service_tokens`, a client secret and an authorization code are both
//! high-entropy, server-generated values — sha256 at rest, never argon2 (same "244 bits of
//! randomness" reasoning as `service_token.rs`); only `user.rs`'s human-chosen passwords
//! need a slow KDF.

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entity::{oauth_clients, oauth_codes, oauth_consent_requests, oauth_consents};

use super::service_token::generate_plaintext;
use super::{StoreError, db_err, json_string_array, sha256_hex, strings_to_json};

#[derive(Debug, Clone)]
pub struct NewOauthClient {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthClient {
    pub id: Uuid,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub scope: Option<String>,
    /// Never the hash itself — only whether one is set, so a caller can tell a confidential
    /// client from a public (PKCE-only) one without any path to the secret.
    pub has_secret: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewOauthCode {
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: Option<String>,
    pub scope: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthCode {
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewConsentRequest {
    pub client_id: Uuid,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub resource: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRequest {
    pub id: Uuid,
    pub client_id: Uuid,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub resource: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct OauthStore {
    // `pub(super)` (visible throughout `store` and its descendants, including the sibling
    // `oauth_tokens` module) rather than private: `oauth_tokens.rs` extends this struct with
    // a second `impl OauthStore` block and needs the connection too — see that module's doc.
    pub(super) db: DatabaseConnection,
}

impl OauthStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Registers a client. `confidential` clients get a server-generated secret, returned
    /// exactly once as the second element (mirrors `service_token::mint`'s plaintext-once
    /// contract); a public (PKCE-only) client gets `None`.
    pub async fn register_client(
        &self,
        new: NewOauthClient,
        confidential: bool,
    ) -> Result<(OauthClient, Option<String>), StoreError> {
        let id = Uuid::new_v4();
        let secret = confidential.then(generate_plaintext);
        oauth_clients::ActiveModel {
            id: Set(id),
            client_secret_hash: Set(secret.as_deref().map(|s| sha256_hex(s.as_bytes()))),
            client_name: Set(new.client_name),
            redirect_uris: Set(strings_to_json(&new.redirect_uris)),
            grant_types: Set(strings_to_json(&new.grant_types)),
            token_endpoint_auth_method: Set(new.token_endpoint_auth_method),
            scope: Set(new.scope),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("oauth::register_client"))?;

        let client = self
            .get_client(id)
            .await?
            .ok_or_else(|| StoreError::Internal("oauth client disappeared after insert".into()))?;
        Ok((client, secret))
    }

    pub async fn get_client(&self, id: Uuid) -> Result<Option<OauthClient>, StoreError> {
        let row = oauth_clients::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("oauth::get_client"))?;
        row.map(client_to_model).transpose()
    }

    pub async fn verify_client_secret(&self, id: Uuid, secret: &str) -> Result<bool, StoreError> {
        let Some(row) = oauth_clients::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("oauth::verify_client_secret"))?
        else {
            return Ok(false);
        };
        Ok(row.client_secret_hash.as_deref() == Some(sha256_hex(secret.as_bytes()).as_str()))
    }

    /// Creates a single-use authorization code, returning its plaintext (stored only as a
    /// hash, same as a service token).
    pub async fn create_code(&self, new: NewOauthCode) -> Result<String, StoreError> {
        let plaintext = generate_plaintext();
        oauth_codes::ActiveModel {
            code_hash: Set(sha256_hex(plaintext.as_bytes())),
            client_id: Set(new.client_id),
            user_id: Set(new.user_id),
            redirect_uri: Set(new.redirect_uri),
            code_challenge: Set(new.code_challenge),
            code_challenge_method: Set(new.code_challenge_method),
            resource: Set(new.resource),
            scope: Set(new.scope),
            used: Set(false),
            expires_at: Set(new.expires_at.into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("oauth::create_code"))?;
        Ok(plaintext)
    }

    /// Redeems a code: `Ok(None)` for "no such code", "already used" and "expired" alike —
    /// same reasoning as `service_token::resolve`. Marks the code used on a successful
    /// redemption; a code is single-use even on the happy path.
    pub async fn consume_code(&self, code: &str) -> Result<Option<OauthCode>, StoreError> {
        let hash = sha256_hex(code.as_bytes());
        let Some(row) = oauth_codes::Entity::find_by_id(hash)
            .one(&self.db)
            .await
            .map_err(db_err("oauth::consume_code"))?
        else {
            return Ok(None);
        };
        if row.used || row.expires_at.with_timezone(&Utc) <= Utc::now() {
            return Ok(None);
        }
        let mut active: oauth_codes::ActiveModel = row.clone().into();
        active.used = Set(true);
        active
            .update(&self.db)
            .await
            .map_err(db_err("oauth::consume_code"))?;
        Ok(Some(OauthCode {
            client_id: row.client_id,
            user_id: row.user_id,
            redirect_uri: row.redirect_uri,
            code_challenge: row.code_challenge,
            code_challenge_method: row.code_challenge_method,
            resource: row.resource,
            scope: row.scope,
        }))
    }

    pub async fn has_consent(
        &self,
        user_id: Uuid,
        client_id: Uuid,
        scope: &str,
    ) -> Result<bool, StoreError> {
        let existing = oauth_consents::Entity::find()
            .filter(oauth_consents::Column::UserId.eq(user_id))
            .filter(oauth_consents::Column::ClientId.eq(client_id))
            .filter(oauth_consents::Column::Scope.eq(scope))
            .one(&self.db)
            .await
            .map_err(db_err("oauth::has_consent"))?;
        Ok(existing.is_some())
    }

    pub async fn grant_consent(
        &self,
        user_id: Uuid,
        client_id: Uuid,
        scope: &str,
    ) -> Result<(), StoreError> {
        if self.has_consent(user_id, client_id, scope).await? {
            return Ok(());
        }
        oauth_consents::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            client_id: Set(client_id),
            scope: Set(scope.to_owned()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("oauth::grant_consent"))?;
        Ok(())
    }

    pub async fn create_consent_request(&self, new: NewConsentRequest) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        oauth_consent_requests::ActiveModel {
            id: Set(id),
            client_id: Set(new.client_id),
            redirect_uri: Set(new.redirect_uri),
            scope: Set(new.scope),
            resource: Set(new.resource),
            code_challenge: Set(new.code_challenge),
            code_challenge_method: Set(new.code_challenge_method),
            state: Set(new.state),
            expires_at: Set(new.expires_at.into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("oauth::create_consent_request"))?;
        Ok(id)
    }

    pub async fn get_consent_request(
        &self,
        id: Uuid,
    ) -> Result<Option<ConsentRequest>, StoreError> {
        let row = oauth_consent_requests::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("oauth::get_consent_request"))?;
        Ok(row.map(|r| ConsentRequest {
            id: r.id,
            client_id: r.client_id,
            redirect_uri: r.redirect_uri,
            scope: r.scope,
            resource: r.resource,
            code_challenge: r.code_challenge,
            code_challenge_method: r.code_challenge_method,
            state: r.state,
            expires_at: r.expires_at.with_timezone(&Utc),
        }))
    }

    pub async fn delete_consent_request(&self, id: Uuid) -> Result<(), StoreError> {
        oauth_consent_requests::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(db_err("oauth::delete_consent_request"))?;
        Ok(())
    }
}

fn client_to_model(row: oauth_clients::Model) -> Result<OauthClient, StoreError> {
    Ok(OauthClient {
        id: row.id,
        client_name: row.client_name,
        redirect_uris: json_string_array(&row.redirect_uris, "oauth_clients.redirect_uris")?,
        grant_types: json_string_array(&row.grant_types, "oauth_clients.grant_types")?,
        token_endpoint_auth_method: row.token_endpoint_auth_method,
        scope: row.scope,
        has_secret: row.client_secret_hash.is_some(),
        created_at: row.created_at.with_timezone(&Utc),
    })
}
