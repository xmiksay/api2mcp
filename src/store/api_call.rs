//! `api_calls` façade. [`ApiCallStore::get`] does the composite read in one pass — the
//! api_call row, its params (ordered by `position`, I7), its service and auth-provider
//! slugs, and its tags — because MCP, the CLI, the read-only API and pack export all need
//! exactly this shape; giving it one home here means the join is written once.
//!
//! `projection`/`pagination` are JSONB; decoding them into
//! [`crate::model::Projection`]/[`crate::model::Pagination`] (and reporting a malformed
//! shape as [`StoreError::Malformed`]) is this module's job, not a caller's.

use std::collections::BTreeSet;
use std::str::FromStr;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::entity::api_calls;
use crate::model::{ApiCall, Slug, Tag};

use super::api_call_params::{load_params, replace_params};
use super::api_call_projection::{
    json_to_pagination, json_to_projection, pagination_to_json, projection_to_json,
};
use super::meta::MetaStore;
use super::{
    AuthProviderStore, ServiceStore, StoreError, TagStore, access_to_str, db_err, json_string_map,
    parse_slug, str_to_access, string_map_to_json,
};

/// An api_call plus the tags it carries. Tags aren't a field of [`ApiCall`] itself (tag
/// membership is a join, not part of an api_call's own definition) — this pairing exists so
/// `resolve::build_plan`'s tag-expression matching can see both at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedApiCall {
    pub api_call: ApiCall,
    pub tags: BTreeSet<Tag>,
}

#[derive(Clone)]
pub struct ApiCallStore {
    db: DatabaseConnection,
}

impl ApiCallStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn get(
        &self,
        service_slug: &Slug,
        slug: &Slug,
    ) -> Result<Option<TaggedApiCall>, StoreError> {
        let service_id = match self.services().id_by_slug(service_slug).await {
            Ok(id) => id,
            Err(StoreError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(row) = api_calls::Entity::find()
            .filter(api_calls::Column::ServiceId.eq(service_id))
            .filter(api_calls::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("api_call::get"))?
        else {
            return Ok(None);
        };
        self.assemble(row, service_slug.clone()).await.map(Some)
    }

    /// Every api_call across every service, for `resolve::build_plan`'s enumeration. Not
    /// paginated: the definition set is operator-curated, not user-generated data.
    pub async fn list_all(&self) -> Result<Vec<TaggedApiCall>, StoreError> {
        let rows = api_calls::Entity::find()
            .order_by_asc(api_calls::Column::Slug)
            .all(&self.db)
            .await
            .map_err(db_err("api_call::list_all"))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let service_slug = self.services().slug_by_id(row.service_id).await?;
            out.push(self.assemble(row, service_slug).await?);
        }
        Ok(out)
    }

    pub async fn create(&self, api_call: &ApiCall, tags: &BTreeSet<Tag>) -> Result<(), StoreError> {
        if self
            .get(&api_call.service_slug, &api_call.slug)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict(format!(
                "api_call {:?} already exists on service {:?}",
                api_call.slug.as_str(),
                api_call.service_slug.as_str()
            )));
        }
        let (service_id, auth_provider_id) = self.resolve_foreign_keys(api_call).await?;
        let id = Uuid::new_v4();
        let txn = self.db.begin().await.map_err(db_err("api_call::create"))?;
        to_active_model(api_call, id, service_id, auth_provider_id)
            .insert(&txn)
            .await
            .map_err(db_err("api_call::create"))?;
        replace_params(&txn, id, &api_call.params).await?;
        TagStore::new(self.db.clone())
            .set_api_call_tags(&txn, id, tags)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("api_call::create"))
    }

    pub async fn update(&self, api_call: &ApiCall, tags: &BTreeSet<Tag>) -> Result<(), StoreError> {
        let id = self
            .id_by_slug(&api_call.service_slug, &api_call.slug)
            .await?;
        let (service_id, auth_provider_id) = self.resolve_foreign_keys(api_call).await?;
        let txn = self.db.begin().await.map_err(db_err("api_call::update"))?;
        to_active_model(api_call, id, service_id, auth_provider_id)
            .update(&txn)
            .await
            .map_err(db_err("api_call::update"))?;
        replace_params(&txn, id, &api_call.params).await?;
        TagStore::new(self.db.clone())
            .set_api_call_tags(&txn, id, tags)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("api_call::update"))
    }

    pub async fn delete(&self, service_slug: &Slug, slug: &Slug) -> Result<(), StoreError> {
        let id = self.id_by_slug(service_slug, slug).await?;
        let txn = self.db.begin().await.map_err(db_err("api_call::delete"))?;
        api_calls::Entity::delete_by_id(id)
            .exec(&txn)
            .await
            .map_err(db_err("api_call::delete"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("api_call::delete"))
    }

    pub(crate) async fn id_by_slug(
        &self,
        service_slug: &Slug,
        slug: &Slug,
    ) -> Result<Uuid, StoreError> {
        let service_id = self.services().id_by_slug(service_slug).await?;
        api_calls::Entity::find()
            .filter(api_calls::Column::ServiceId.eq(service_id))
            .filter(api_calls::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("api_call::id_by_slug"))?
            .map(|r| r.id)
            .ok_or(StoreError::NotFound)
    }

    /// Resolves a bare api_call slug to its primary key, with no service to scope by —
    /// needed by `script_api_calls` writes, since `ScriptDef::callable` (a fixed part of
    /// the model) maps an alias to a bare [`Slug`], not a `(service, slug)` pair. Safe
    /// because `api_calls.slug` is `UNIQUE` globally (`ux_api_calls_slug`), so at most one
    /// row can ever match.
    pub(crate) async fn id_by_slug_global(&self, slug: &Slug) -> Result<Uuid, StoreError> {
        api_calls::Entity::find()
            .filter(api_calls::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("api_call::id_by_slug_global"))?
            .map(|r| r.id)
            .ok_or(StoreError::NotFound)
    }

    /// Resolves an api_call's `(service_slug, slug)` pair from its primary key — used by
    /// `store::script` to turn a `script_api_calls.api_call_id` foreign key into the bare
    /// [`Slug`] `ScriptDef::callable` carries.
    pub(crate) async fn slug_by_id(&self, id: Uuid) -> Result<Slug, StoreError> {
        let row = api_calls::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("api_call::slug_by_id"))?
            .ok_or(StoreError::NotFound)?;
        parse_slug(&row.slug)
    }

    async fn assemble(
        &self,
        row: api_calls::Model,
        service_slug: Slug,
    ) -> Result<TaggedApiCall, StoreError> {
        let id = row.id;
        let auth_provider_slug = match row.auth_provider_id {
            Some(aid) => Some(self.auth_providers().slug_by_id(aid).await?),
            None => None,
        };
        let params = load_params(&self.db, id).await?;
        let tags = TagStore::new(self.db.clone()).tags_for_api_call(id).await?;
        let api_call = to_model(row, service_slug, auth_provider_slug, params)?;
        Ok(TaggedApiCall { api_call, tags })
    }

    async fn resolve_foreign_keys(
        &self,
        api_call: &ApiCall,
    ) -> Result<(Uuid, Option<Uuid>), StoreError> {
        let service_id = self.services().id_by_slug(&api_call.service_slug).await?;
        let auth_provider_id = match &api_call.auth_provider_slug {
            Some(slug) => Some(
                self.auth_providers()
                    .id_by_slug(&api_call.service_slug, slug)
                    .await?,
            ),
            None => None,
        };
        Ok((service_id, auth_provider_id))
    }

    fn services(&self) -> ServiceStore {
        ServiceStore::new(self.db.clone())
    }

    fn auth_providers(&self) -> AuthProviderStore {
        AuthProviderStore::new(self.db.clone())
    }
}

fn to_active_model(
    api_call: &ApiCall,
    id: Uuid,
    service_id: Uuid,
    auth_provider_id: Option<Uuid>,
) -> api_calls::ActiveModel {
    api_calls::ActiveModel {
        id: Set(id),
        service_id: Set(service_id),
        auth_provider_id: Set(auth_provider_id),
        slug: Set(api_call.slug.as_str().to_owned()),
        method: Set(api_call.method.to_string()),
        path_template: Set(api_call.path_template.clone()),
        query_fixed: Set(string_map_to_json(&api_call.query_fixed)),
        body_template: Set(api_call.body_template.clone()),
        access: Set(access_to_str(api_call.access).to_owned()),
        idempotent: Set(api_call.idempotent),
        projection: Set(api_call.projection.as_ref().map(projection_to_json)),
        pagination: Set(pagination_to_json(&api_call.pagination)),
        timeout_ms: Set(api_call.timeout_ms.map(|v| v as i32)),
        max_response_bytes: Set(api_call.max_response_bytes.map(|v| v as i64)),
        ..Default::default()
    }
}

fn to_model(
    row: api_calls::Model,
    service_slug: Slug,
    auth_provider_slug: Option<Slug>,
    params: Vec<crate::model::Param>,
) -> Result<ApiCall, StoreError> {
    let access = str_to_access(&row.access)?;
    let method = http::Method::from_str(&row.method)
        .map_err(|e| StoreError::Malformed(format!("api_calls.method {:?}: {e}", row.method)))?;
    let projection = row
        .projection
        .as_ref()
        .map(json_to_projection)
        .transpose()?;
    let pagination = json_to_pagination(row.pagination.as_ref())?;

    Ok(ApiCall {
        slug: parse_slug(&row.slug)?,
        service_slug,
        auth_provider_slug,
        method,
        path_template: row.path_template,
        query_fixed: json_string_map(&row.query_fixed, "api_calls.query_fixed")?,
        body_template: row.body_template,
        access,
        idempotent: row.idempotent,
        projection,
        pagination,
        timeout_ms: row.timeout_ms.map(|v| v as u32),
        max_response_bytes: row.max_response_bytes.map(|v| v as u64),
        params,
    })
}
