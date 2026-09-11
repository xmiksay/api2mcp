//! `scripts` façade. [`ScriptStore::get`] mirrors `api_call.rs`'s composite read: the
//! script row, its `script_params`, its `script_api_calls` allowlist (I1's *declarative*
//! half — the fixed, human-curated set of api_calls a script may reach, resolved here into
//! [`crate::model::ScriptDef::callable`]) and its tags.
//!
//! `scripts` stores its own [`crate::model::Budgets`] opinion (I6) as five nullable
//! columns, one per axis, rather than one JSONB blob — see the migration's module doc for
//! why every axis needs to be independently representable here.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{script_api_calls, script_params, scripts};
use crate::model::{Param, ParamLocation, ScriptDef, Slug, Tag};

use super::api_call::ApiCallStore;
use super::api_call_params::{data_type_to_str, str_to_data_type};
use super::meta::MetaStore;
use super::{StoreError, TagStore, db_err, enum_values_to_json, json_to_enum_values, parse_slug};

/// A script plus the tags it carries — see [`super::api_call::TaggedApiCall`] for why this
/// pairing lives outside `model::ScriptDef` itself.
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedScript {
    pub script: ScriptDef,
    pub tags: BTreeSet<Tag>,
}

#[derive(Clone)]
pub struct ScriptStore {
    db: DatabaseConnection,
}

impl ScriptStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn get(&self, slug: &Slug) -> Result<Option<TaggedScript>, StoreError> {
        let Some(row) = scripts::Entity::find()
            .filter(scripts::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("script::get"))?
        else {
            return Ok(None);
        };
        self.assemble(row).await.map(Some)
    }

    pub async fn list_all(&self) -> Result<Vec<TaggedScript>, StoreError> {
        let rows = scripts::Entity::find()
            .order_by_asc(scripts::Column::Slug)
            .all(&self.db)
            .await
            .map_err(db_err("script::list_all"))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.assemble(row).await?);
        }
        Ok(out)
    }

    pub async fn create(&self, script: &ScriptDef, tags: &BTreeSet<Tag>) -> Result<(), StoreError> {
        if self.get(&script.slug).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "script {:?} already exists",
                script.slug.as_str()
            )));
        }
        let id = Uuid::new_v4();
        let txn = self.db.begin().await.map_err(db_err("script::create"))?;
        to_active_model(script, id)
            .insert(&txn)
            .await
            .map_err(db_err("script::create"))?;
        replace_params(&txn, id, &script.params).await?;
        self.replace_callable(&txn, id, &script.callable).await?;
        TagStore::new(self.db.clone())
            .set_script_tags(id, tags)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("script::create"))
    }

    pub async fn update(&self, script: &ScriptDef, tags: &BTreeSet<Tag>) -> Result<(), StoreError> {
        let id = self.id_by_slug(&script.slug).await?;
        let txn = self.db.begin().await.map_err(db_err("script::update"))?;
        to_active_model(script, id)
            .update(&txn)
            .await
            .map_err(db_err("script::update"))?;
        replace_params(&txn, id, &script.params).await?;
        self.replace_callable(&txn, id, &script.callable).await?;
        TagStore::new(self.db.clone())
            .set_script_tags(id, tags)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("script::update"))
    }

    pub async fn delete(&self, slug: &Slug) -> Result<(), StoreError> {
        let id = self.id_by_slug(slug).await?;
        let txn = self.db.begin().await.map_err(db_err("script::delete"))?;
        scripts::Entity::delete_by_id(id)
            .exec(&txn)
            .await
            .map_err(db_err("script::delete"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("script::delete"))
    }

    pub(crate) async fn id_by_slug(&self, slug: &Slug) -> Result<Uuid, StoreError> {
        scripts::Entity::find()
            .filter(scripts::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("script::id_by_slug"))?
            .map(|r| r.id)
            .ok_or(StoreError::NotFound)
    }

    async fn assemble(&self, row: scripts::Model) -> Result<TaggedScript, StoreError> {
        let id = row.id;
        let params = load_script_params(&self.db, id).await?;
        let callable = self.load_callable(id).await?;
        let tags = TagStore::new(self.db.clone()).tags_for_script(id).await?;
        let script = to_model(row, params, callable)?;
        Ok(TaggedScript { script, tags })
    }

    async fn load_callable(&self, script_id: Uuid) -> Result<BTreeMap<String, Slug>, StoreError> {
        let rows = script_api_calls::Entity::find()
            .filter(script_api_calls::Column::ScriptId.eq(script_id))
            .all(&self.db)
            .await
            .map_err(db_err("script::load_callable"))?;
        let api_calls = ApiCallStore::new(self.db.clone());
        let mut callable = BTreeMap::new();
        for row in rows {
            let slug = api_calls.slug_by_id(row.api_call_id).await?;
            callable.insert(row.alias, slug);
        }
        Ok(callable)
    }

    async fn replace_callable<C: ConnectionTrait>(
        &self,
        conn: &C,
        script_id: Uuid,
        callable: &BTreeMap<String, Slug>,
    ) -> Result<(), StoreError> {
        script_api_calls::Entity::delete_many()
            .filter(script_api_calls::Column::ScriptId.eq(script_id))
            .exec(conn)
            .await
            .map_err(db_err("script::replace_callable"))?;
        let api_calls = ApiCallStore::new(self.db.clone());
        for (alias, api_call_slug) in callable {
            let api_call_id = api_calls.id_by_slug_global(api_call_slug).await?;
            script_api_calls::ActiveModel {
                id: Set(Uuid::new_v4()),
                script_id: Set(script_id),
                api_call_id: Set(api_call_id),
                alias: Set(alias.clone()),
            }
            .insert(conn)
            .await
            .map_err(db_err("script::replace_callable"))?;
        }
        Ok(())
    }
}

fn to_active_model(script: &ScriptDef, id: Uuid) -> scripts::ActiveModel {
    let b = &script.budgets;
    scripts::ActiveModel {
        id: Set(id),
        slug: Set(script.slug.as_str().to_owned()),
        description: Set(script.description.clone()),
        source: Set(script.source.clone()),
        max_calls: Set(b.max_calls.map(|v| v as i32)),
        max_bytes: Set(b.max_bytes.map(|v| v as i64)),
        wall_clock_ms: Set(b
            .wall_clock
            .map(|d| d.as_millis().min(i32::MAX as u128) as i32)),
        max_pages: Set(b.max_pages.map(|v| v as i32)),
        max_concurrency: Set(b.max_concurrency.map(|v| v as i32)),
        ..Default::default()
    }
}

fn to_model(
    row: scripts::Model,
    params: Vec<Param>,
    callable: BTreeMap<String, Slug>,
) -> Result<ScriptDef, StoreError> {
    Ok(ScriptDef {
        slug: parse_slug(&row.slug)?,
        source: row.source,
        params,
        callable,
        budgets: crate::model::Budgets {
            max_calls: row.max_calls.map(|v| v.max(0) as u32),
            max_bytes: row.max_bytes.map(|v| v.max(0) as u64),
            wall_clock: row
                .wall_clock_ms
                .map(|ms| Duration::from_millis(ms.max(0) as u64)),
            max_pages: row.max_pages.map(|v| v.max(0) as u32),
            max_concurrency: row.max_concurrency.map(|v| v.max(0) as u32),
        },
        description: row.description,
    })
}

/// Deletes and re-inserts every `script_params` row for `script_id`. A script param's
/// `location` must be [`ParamLocation::Local`] — `script_params` has no `location` column,
/// so any other location would be silently dropped; rejecting it here surfaces the bug at
/// write time instead.
async fn replace_params<C: ConnectionTrait>(
    conn: &C,
    script_id: Uuid,
    params: &[Param],
) -> Result<(), StoreError> {
    script_params::Entity::delete_many()
        .filter(script_params::Column::ScriptId.eq(script_id))
        .exec(conn)
        .await
        .map_err(db_err("script::replace_params"))?;
    for p in params {
        if p.location != ParamLocation::Local {
            return Err(StoreError::Conflict(format!(
                "script param {:?} must use ParamLocation::Local (script_params has no location column)",
                p.name
            )));
        }
        script_params::ActiveModel {
            id: Set(Uuid::new_v4()),
            script_id: Set(script_id),
            name: Set(p.name.clone()),
            data_type: Set(data_type_to_str(p.ty).to_owned()),
            required: Set(p.required),
            default_value: Set(p.default.clone()),
            enum_values: Set(enum_values_to_json(&p.enum_values)),
            position: Set(p.position),
            description: Set(p.description.clone().unwrap_or_default()),
        }
        .insert(conn)
        .await
        .map_err(db_err("script::replace_params"))?;
    }
    Ok(())
}

async fn load_script_params<C: ConnectionTrait>(
    conn: &C,
    script_id: Uuid,
) -> Result<Vec<Param>, StoreError> {
    let rows = script_params::Entity::find()
        .filter(script_params::Column::ScriptId.eq(script_id))
        .order_by_asc(script_params::Column::Position)
        .all(conn)
        .await
        .map_err(db_err("script::load_script_params"))?;
    rows.into_iter()
        .map(|row| {
            Ok(Param {
                name: row.name,
                location: ParamLocation::Local,
                ty: str_to_data_type(&row.data_type)?,
                required: row.required,
                default: row.default_value,
                fixed: None,
                enum_values: json_to_enum_values(row.enum_values, "script_params.enum_values")?,
                description: (!row.description.is_empty()).then_some(row.description),
                position: row.position,
            })
        })
        .collect()
}
