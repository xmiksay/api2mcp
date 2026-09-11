//! `service_tokens` façade — long-lived MCP bearer tokens. [`ServiceTokenStore::mint`]
//! returns the plaintext exactly once; only `sha256(token)` and an 8-char display prefix are
//! ever persisted. [`ServiceTokenStore::resolve`] is the only path that turns a caller-
//! presented token back into a record, and it rejects a revoked or expired one outright
//! rather than returning it with a flag a caller might forget to check.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set,
};
use uuid::Uuid;

use crate::entity::service_tokens;

use super::{StoreError, db_err, json_string_array, sha256_hex, strings_to_json};

/// A freshly minted token. `plaintext` exists only in this struct — it is never stored, and
/// this is the only place in the crate it is ever produced.
#[derive(Debug, Clone)]
pub struct MintedServiceToken {
    pub record: ServiceTokenRecord,
    pub plaintext: String,
}

/// Never carries `token_hash`: [`ServiceTokenStore::list`] must not be able to leak anything
/// secret-adjacent, even a hash, so the type it returns simply has nowhere to put one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTokenRecord {
    pub id: Uuid,
    pub token_prefix: String,
    pub owner_id: Uuid,
    pub label: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct ServiceTokenStore {
    db: DatabaseConnection,
}

impl ServiceTokenStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn mint(
        &self,
        owner_id: Uuid,
        label: String,
        scopes: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<MintedServiceToken, StoreError> {
        let plaintext = generate_plaintext();
        let id = Uuid::new_v4();
        service_tokens::ActiveModel {
            id: Set(id),
            token_hash: Set(sha256_hex(plaintext.as_bytes())),
            token_prefix: Set(display_prefix(&plaintext)),
            owner_id: Set(owner_id),
            label: Set(label),
            scopes: Set(strings_to_json(&scopes)),
            last_used_at: Set(None),
            expires_at: Set(expires_at.map(Into::into)),
            revoked_at: Set(None),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(db_err("service_token::mint"))?;

        let record = self
            .get(id)
            .await?
            .ok_or_else(|| StoreError::Internal("service token disappeared after insert".into()))?;
        Ok(MintedServiceToken { record, plaintext })
    }

    /// Resolves a caller-presented plaintext token by its hash. `Ok(None)` for "no such
    /// token", "revoked" and "expired" alike — a caller must not be able to distinguish
    /// those from this method's return shape (that distinction belongs in an audit log, not
    /// in the auth decision). Bumps `last_used_at` on every successful resolution.
    pub async fn resolve(&self, plaintext: &str) -> Result<Option<ServiceTokenRecord>, StoreError> {
        let hash = sha256_hex(plaintext.as_bytes());
        let Some(row) = service_tokens::Entity::find()
            .filter(service_tokens::Column::TokenHash.eq(hash))
            .one(&self.db)
            .await
            .map_err(db_err("service_token::resolve"))?
        else {
            return Ok(None);
        };
        if row.revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(expires_at) = row.expires_at
            && expires_at.with_timezone(&Utc) <= Utc::now()
        {
            return Ok(None);
        }

        let now = Utc::now();
        let mut active = row.clone().into_active_model();
        active.last_used_at = Set(Some(now.into()));
        active
            .update(&self.db)
            .await
            .map_err(db_err("service_token::resolve"))?;

        let mut record = to_model(row)?;
        record.last_used_at = Some(now);
        Ok(Some(record))
    }

    pub async fn list_for_owner(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<ServiceTokenRecord>, StoreError> {
        let rows = service_tokens::Entity::find()
            .filter(service_tokens::Column::OwnerId.eq(owner_id))
            .order_by_asc(service_tokens::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(db_err("service_token::list_for_owner"))?;
        rows.into_iter().map(to_model).collect()
    }

    pub async fn revoke(&self, id: Uuid) -> Result<(), StoreError> {
        let row = service_tokens::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("service_token::revoke"))?
            .ok_or(StoreError::NotFound)?;
        let mut active = row.into_active_model();
        active.revoked_at = Set(Some(Utc::now().into()));
        active
            .update(&self.db)
            .await
            .map_err(db_err("service_token::revoke"))?;
        Ok(())
    }

    async fn get(&self, id: Uuid) -> Result<Option<ServiceTokenRecord>, StoreError> {
        let row = service_tokens::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("service_token::get"))?;
        row.map(to_model).transpose()
    }
}

/// High-entropy plaintext generator shared with `store::oauth` (client secrets and
/// authorization codes are the same "server-generated, sha256-at-rest" shape as a service
/// token).
pub(crate) fn generate_plaintext() -> String {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn display_prefix(plaintext: &str) -> String {
    plaintext.chars().take(8).collect()
}

fn to_model(row: service_tokens::Model) -> Result<ServiceTokenRecord, StoreError> {
    Ok(ServiceTokenRecord {
        id: row.id,
        token_prefix: row.token_prefix,
        owner_id: row.owner_id,
        label: row.label,
        scopes: json_string_array(&row.scopes, "service_tokens.scopes")?,
        last_used_at: row.last_used_at.map(|d| d.with_timezone(&Utc)),
        expires_at: row.expires_at.map(|d| d.with_timezone(&Utc)),
        revoked_at: row.revoked_at.map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.with_timezone(&Utc),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefix_never_reveals_the_full_token() {
        let plaintext = generate_plaintext();
        assert_eq!(display_prefix(&plaintext).len(), 8);
        assert_ne!(display_prefix(&plaintext), plaintext);
    }

    #[test]
    fn generated_tokens_are_unique() {
        assert_ne!(generate_plaintext(), generate_plaintext());
    }
}
