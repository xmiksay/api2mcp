//! `service_tokens` façade — long-lived MCP bearer tokens. [`ServiceTokenStore::mint`]
//! returns the plaintext exactly once; only `sha256(token)` and an 8-char display prefix are
//! ever persisted. [`ServiceTokenStore::resolve`] is the only path that turns a caller-
//! presented token back into a record, and it rejects a revoked or expired one outright
//! rather than returning it with a flag a caller might forget to check.
//!
//! **Endpoint grants** (`service_token_endpoints`, `migration::m0005_endpoints`): a token
//! minted with an empty endpoint set can reach every endpoint (the permissive default,
//! equivalent to a GitHub classic PAT); a non-empty set restricts it to exactly those,
//! like a fine-grained one. [`ServiceTokenRecord::endpoints`] carries that set as `Slug`s —
//! empty means unrestricted, the same "empty = all" convention the wire contract
//! (`server::api::tokens`) uses, so no separate `Option`/enum is needed anywhere this value
//! travels. Enforcing the restriction at request time is `server::auth::authenticate_mcp`'s
//! job (it reads `resolve`'s output), not this module's.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{service_token_endpoints, service_tokens};
use crate::model::Slug;

use super::endpoint::EndpointStore;
use super::{StoreError, db_err, sha256_hex};

/// A freshly minted token. `plaintext` exists only in this struct — it is never stored, and
/// this is the only place in the crate it is ever produced.
#[derive(Debug, Clone)]
pub struct MintedServiceToken {
    pub record: ServiceTokenRecord,
    pub plaintext: String,
}

/// Never carries `token_hash`: [`ServiceTokenStore::list_for_owner`] must not be able to leak
/// anything secret-adjacent, even a hash, so the type it returns simply has nowhere to put one.
///
/// No `scopes` any more: it used to hold `"mcp"` or `"admin"`, but the admin/non-admin
/// distinction is gone (see `server::identity`'s module doc), which left exactly one possible
/// value — a column that can only ever hold one value encodes nothing, so it was dropped
/// (`migration::m0001_init`) rather than kept as a decoration. A resolved, unrevoked,
/// unexpired service token may call tools over `/mcp`, restricted to `endpoints` when that set
/// is non-empty; that's the only rule left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTokenRecord {
    pub id: Uuid,
    pub token_prefix: String,
    pub owner_id: Uuid,
    pub label: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Endpoint slugs this token is restricted to. Empty means unrestricted — see this
    /// module's own doc.
    pub endpoints: BTreeSet<Slug>,
}

#[derive(Clone)]
pub struct ServiceTokenStore {
    db: DatabaseConnection,
}

impl ServiceTokenStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Mints a token restricted to `endpoints` (empty = every endpoint). An unknown slug in
    /// `endpoints` is [`StoreError::Conflict`] — a definer/caller error the DB schema can't
    /// itself catch (there is nothing to insert an FK-valid row against) — rather than a raw
    /// foreign-key violation or a silently-dropped grant.
    pub async fn mint(
        &self,
        owner_id: Uuid,
        label: String,
        expires_at: Option<DateTime<Utc>>,
        endpoints: BTreeSet<Slug>,
    ) -> Result<MintedServiceToken, StoreError> {
        let plaintext = generate_plaintext();
        let id = Uuid::new_v4();
        let endpoint_ids = self.resolve_endpoint_ids(&endpoints).await?;

        let txn = self
            .db
            .begin()
            .await
            .map_err(db_err("service_token::mint"))?;
        service_tokens::ActiveModel {
            id: Set(id),
            token_hash: Set(sha256_hex(plaintext.as_bytes())),
            token_prefix: Set(display_prefix(&plaintext)),
            owner_id: Set(owner_id),
            label: Set(label),
            last_used_at: Set(None),
            expires_at: Set(expires_at.map(Into::into)),
            revoked_at: Set(None),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(db_err("service_token::mint"))?;
        for endpoint_id in &endpoint_ids {
            service_token_endpoints::ActiveModel {
                service_token_id: Set(id),
                endpoint_id: Set(*endpoint_id),
            }
            .insert(&txn)
            .await
            .map_err(db_err("service_token::mint"))?;
        }
        txn.commit().await.map_err(db_err("service_token::mint"))?;

        let record = self
            .get_by_id(id)
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
        let id = row.id;
        let mut active = row.clone().into_active_model();
        active.last_used_at = Set(Some(now.into()));
        active
            .update(&self.db)
            .await
            .map_err(db_err("service_token::resolve"))?;

        let endpoints = self.load_endpoint_grants(id).await?;
        let mut record = to_model(row, endpoints);
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
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let endpoints = self.load_endpoint_grants(row.id).await?;
            out.push(to_model(row, endpoints));
        }
        Ok(out)
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

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<ServiceTokenRecord>, StoreError> {
        let Some(row) = service_tokens::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("service_token::get_by_id"))?
        else {
            return Ok(None);
        };
        let endpoints = self.load_endpoint_grants(id).await?;
        Ok(Some(to_model(row, endpoints)))
    }

    async fn load_endpoint_grants(&self, token_id: Uuid) -> Result<BTreeSet<Slug>, StoreError> {
        let rows = service_token_endpoints::Entity::find()
            .filter(service_token_endpoints::Column::ServiceTokenId.eq(token_id))
            .all(&self.db)
            .await
            .map_err(db_err("service_token::load_endpoint_grants"))?;
        let endpoints = EndpointStore::new(self.db.clone());
        let mut out = BTreeSet::new();
        for row in rows {
            out.insert(endpoints.slug_by_id(row.endpoint_id).await?);
        }
        Ok(out)
    }

    async fn resolve_endpoint_ids(
        &self,
        slugs: &BTreeSet<Slug>,
    ) -> Result<BTreeSet<Uuid>, StoreError> {
        let endpoints = EndpointStore::new(self.db.clone());
        let mut ids = BTreeSet::new();
        for slug in slugs {
            let id = endpoints.id_by_slug(slug).await.map_err(|e| match e {
                StoreError::NotFound => {
                    StoreError::Conflict(format!("unknown endpoint {:?}", slug.as_str()))
                }
                other => other,
            })?;
            ids.insert(id);
        }
        Ok(ids)
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

fn to_model(row: service_tokens::Model, endpoints: BTreeSet<Slug>) -> ServiceTokenRecord {
    ServiceTokenRecord {
        id: row.id,
        token_prefix: row.token_prefix,
        owner_id: row.owner_id,
        label: row.label,
        last_used_at: row.last_used_at.map(|d| d.with_timezone(&Utc)),
        expires_at: row.expires_at.map(|d| d.with_timezone(&Utc)),
        revoked_at: row.revoked_at.map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.with_timezone(&Utc),
        endpoints,
    }
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
