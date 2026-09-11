//! `services` façade — the upstream API definition: base URL, reachable-origin allowlist
//! (I2), default headers, and the connection-level limits [`crate::runtime::budget`] folds
//! against.
//!
//! `Service::rate_limit_per_min` is `Option<u32>` ("no opinion" means `None`), but the
//! `rate_limit_per_min` column is `NOT NULL INTEGER` — there is no schema-level NULL to
//! round-trip through. `0` is the sentinel for "no opinion" in both directions; a service
//! that genuinely wants to rate-limit itself to zero requests/minute would simply disable
//! itself instead, so the sentinel doesn't collide with a meaningful value.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::entity::services;
use crate::model::{Service, Slug};

use super::meta::MetaStore;
use super::{
    StoreError, db_err, json_origin_set, json_string_map, origins_to_json, parse_slug, parse_url,
    string_map_to_json,
};

#[derive(Clone)]
pub struct ServiceStore {
    db: DatabaseConnection,
}

impl ServiceStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Inserts a new service row. [`StoreError::Conflict`] if `service.slug` is already
    /// taken — surfacing the unique-index violation as a typed error instead of a bare
    /// [`StoreError::Db`].
    pub async fn create(&self, service: &Service) -> Result<(), StoreError> {
        if self.get_by_slug(&service.slug).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "service {:?} already exists",
                service.slug.as_str()
            )));
        }
        let txn = self.db.begin().await.map_err(db_err("service::create"))?;
        to_active_model(service, Uuid::new_v4())
            .insert(&txn)
            .await
            .map_err(db_err("service::create"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("service::create"))
    }

    /// Replaces every field of the service identified by `service.slug`.
    pub async fn update(&self, service: &Service) -> Result<(), StoreError> {
        let id = self.id_by_slug(&service.slug).await?;
        let txn = self.db.begin().await.map_err(db_err("service::update"))?;
        let mut active = to_active_model(service, id);
        active.updated_at = Set(Utc::now().into());
        active
            .update(&txn)
            .await
            .map_err(db_err("service::update"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("service::update"))
    }

    pub async fn get_by_slug(&self, slug: &Slug) -> Result<Option<Service>, StoreError> {
        let row = services::Entity::find()
            .filter(services::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("service::get_by_slug"))?;
        row.map(to_model).transpose()
    }

    pub async fn list(&self) -> Result<Vec<Service>, StoreError> {
        let rows = services::Entity::find()
            .order_by_asc(services::Column::Slug)
            .all(&self.db)
            .await
            .map_err(db_err("service::list"))?;
        rows.into_iter().map(to_model).collect()
    }

    pub async fn delete(&self, slug: &Slug) -> Result<(), StoreError> {
        let id = self.id_by_slug(slug).await?;
        let txn = self.db.begin().await.map_err(db_err("service::delete"))?;
        services::Entity::delete_by_id(id)
            .exec(&txn)
            .await
            .map_err(db_err("service::delete"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("service::delete"))
    }

    /// Resolves a service's primary key from its slug — an internal join helper for
    /// sibling stores (`api_call`, `auth_provider`); a `Uuid` is a plain scalar, never an
    /// `entity::Model`.
    pub(crate) async fn id_by_slug(&self, slug: &Slug) -> Result<Uuid, StoreError> {
        services::Entity::find()
            .filter(services::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("service::id_by_slug"))?
            .map(|row| row.id)
            .ok_or(StoreError::NotFound)
    }

    /// Resolves a service's slug from its primary key — the reverse of [`Self::id_by_slug`],
    /// used when a sibling store joins back from a foreign key to build a `model::` type
    /// that only ever carries the slug, never the internal id.
    pub(crate) async fn slug_by_id(&self, id: Uuid) -> Result<Slug, StoreError> {
        let row = services::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("service::slug_by_id"))?
            .ok_or(StoreError::NotFound)?;
        parse_slug(&row.slug)
    }
}

fn to_active_model(service: &Service, id: Uuid) -> services::ActiveModel {
    services::ActiveModel {
        id: Set(id),
        slug: Set(service.slug.as_str().to_owned()),
        base_url: Set(service.base_url.to_string()),
        origin_allowlist: Set(origins_to_json(&service.origin_allowlist)),
        default_headers: Set(string_map_to_json(&service.default_headers)),
        timeout_ms: Set(service.timeout_ms as i32),
        max_concurrency: Set(service.max_concurrency as i32),
        rate_limit_per_min: Set(service.rate_limit_per_min.unwrap_or(0) as i32),
        max_response_bytes: Set(service.max_response_bytes as i64),
        ..Default::default()
    }
}

fn to_model(row: services::Model) -> Result<Service, StoreError> {
    Ok(Service {
        slug: parse_slug(&row.slug)?,
        base_url: parse_url(&row.base_url)?,
        origin_allowlist: json_origin_set(&row.origin_allowlist, "services.origin_allowlist")?,
        default_headers: json_string_map(&row.default_headers, "services.default_headers")?,
        timeout_ms: row.timeout_ms as u32,
        max_concurrency: row.max_concurrency as u32,
        rate_limit_per_min: if row.rate_limit_per_min <= 0 {
            None
        } else {
            Some(row.rate_limit_per_min as u32)
        },
        max_response_bytes: row.max_response_bytes as u64,
    })
}
