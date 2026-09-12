//! `endpoints` façade: a tag-expression selection of tools, its aliases, its write ceiling,
//! its folded budget and which auth providers it may bind (I5's other human-set join —
//! `endpoint_auth_providers`).
//!
//! `model::EndpointDef::tag_expr` is already the parsed [`crate::model::TagExpr`] AST, not
//! the raw string the `tag_expr` column holds; parsing/printing that string is
//! [`crate::resolve::tag_expr`]'s job (promoted there from this module in chunk C6), not
//! this store's — this module only calls into it at the row <-> model boundary.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::{endpoint_aliases, endpoint_auth_providers, endpoints};
use crate::model::{Budgets, EndpointDef, EndpointTarget, Slug};

use crate::resolve::tag_expr;

use super::auth_provider::AuthProviderStore;
use super::meta::MetaStore;
use super::{StoreError, access_to_str, db_err, parse_slug, str_to_access};

#[derive(Clone)]
pub struct EndpointStore {
    db: DatabaseConnection,
}

impl EndpointStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn get(&self, slug: &Slug) -> Result<Option<EndpointDef>, StoreError> {
        let Some(row) = endpoints::Entity::find()
            .filter(endpoints::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("endpoint::get"))?
        else {
            return Ok(None);
        };
        self.assemble(row).await.map(Some)
    }

    pub async fn list_all(&self) -> Result<Vec<EndpointDef>, StoreError> {
        let rows = endpoints::Entity::find()
            .order_by_asc(endpoints::Column::Slug)
            .all(&self.db)
            .await
            .map_err(db_err("endpoint::list_all"))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.assemble(row).await?);
        }
        Ok(out)
    }

    pub async fn create(&self, endpoint: &EndpointDef) -> Result<(), StoreError> {
        if self.get(&endpoint.slug).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "endpoint {:?} already exists",
                endpoint.slug.as_str()
            )));
        }
        let id = Uuid::new_v4();
        let txn = self.db.begin().await.map_err(db_err("endpoint::create"))?;
        to_active_model(endpoint, id)?
            .insert(&txn)
            .await
            .map_err(db_err("endpoint::create"))?;
        self.replace_aliases(&txn, id, &endpoint.aliases).await?;
        self.replace_auth_providers(&txn, id, &endpoint.auth_providers)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("endpoint::create"))
    }

    pub async fn update(&self, endpoint: &EndpointDef) -> Result<(), StoreError> {
        let id = self.id_by_slug(&endpoint.slug).await?;
        let txn = self.db.begin().await.map_err(db_err("endpoint::update"))?;
        to_active_model(endpoint, id)?
            .update(&txn)
            .await
            .map_err(db_err("endpoint::update"))?;
        self.replace_aliases(&txn, id, &endpoint.aliases).await?;
        self.replace_auth_providers(&txn, id, &endpoint.auth_providers)
            .await?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("endpoint::update"))
    }

    pub async fn delete(&self, slug: &Slug) -> Result<(), StoreError> {
        let id = self.id_by_slug(slug).await?;
        let txn = self.db.begin().await.map_err(db_err("endpoint::delete"))?;
        endpoints::Entity::delete_by_id(id)
            .exec(&txn)
            .await
            .map_err(db_err("endpoint::delete"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("endpoint::delete"))
    }

    pub(crate) async fn id_by_slug(&self, slug: &Slug) -> Result<Uuid, StoreError> {
        endpoints::Entity::find()
            .filter(endpoints::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("endpoint::id_by_slug"))?
            .map(|r| r.id)
            .ok_or(StoreError::NotFound)
    }

    /// The inverse of [`Self::id_by_slug`] — used by `store::service_token` to turn a service
    /// token's `service_token_endpoints` rows back into the `Slug`s its grant list is expressed
    /// in everywhere else in the crate.
    pub(crate) async fn slug_by_id(&self, id: Uuid) -> Result<Slug, StoreError> {
        endpoints::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("endpoint::slug_by_id"))?
            .ok_or(StoreError::NotFound)
            .and_then(|row| parse_slug(&row.slug))
    }

    async fn assemble(&self, row: endpoints::Model) -> Result<EndpointDef, StoreError> {
        let id = row.id;
        let aliases = self.load_aliases(id).await?;
        let auth_providers = self.load_auth_providers(id).await?;
        to_model(row, aliases, auth_providers)
    }

    async fn load_aliases(
        &self,
        endpoint_id: Uuid,
    ) -> Result<BTreeMap<String, EndpointTarget>, StoreError> {
        let rows = endpoint_aliases::Entity::find()
            .filter(endpoint_aliases::Column::EndpointId.eq(endpoint_id))
            .all(&self.db)
            .await
            .map_err(db_err("endpoint::load_aliases"))?;
        rows.into_iter()
            .map(|r| {
                let target_slug = parse_slug(&r.target_slug)?;
                let target = match r.target_kind.as_str() {
                    "api_call" => EndpointTarget::ApiCall(target_slug),
                    "script" => EndpointTarget::Script(target_slug),
                    other => {
                        return Err(StoreError::Malformed(format!(
                            "endpoint_aliases.target_kind: unrecognised value {other:?}"
                        )));
                    }
                };
                Ok((r.alias, target))
            })
            .collect()
    }

    async fn load_auth_providers(&self, endpoint_id: Uuid) -> Result<BTreeSet<Slug>, StoreError> {
        let rows = endpoint_auth_providers::Entity::find()
            .filter(endpoint_auth_providers::Column::EndpointId.eq(endpoint_id))
            .all(&self.db)
            .await
            .map_err(db_err("endpoint::load_auth_providers"))?;
        let providers = AuthProviderStore::new(self.db.clone());
        let mut out = BTreeSet::new();
        for row in rows {
            out.insert(providers.slug_by_id(row.auth_provider_id).await?);
        }
        Ok(out)
    }

    async fn replace_aliases<C: ConnectionTrait>(
        &self,
        conn: &C,
        endpoint_id: Uuid,
        aliases: &BTreeMap<String, EndpointTarget>,
    ) -> Result<(), StoreError> {
        endpoint_aliases::Entity::delete_many()
            .filter(endpoint_aliases::Column::EndpointId.eq(endpoint_id))
            .exec(conn)
            .await
            .map_err(db_err("endpoint::replace_aliases"))?;
        for (alias, target) in aliases {
            let (target_kind, target_slug) = match target {
                EndpointTarget::ApiCall(slug) => ("api_call", slug.as_str()),
                EndpointTarget::Script(slug) => ("script", slug.as_str()),
            };
            endpoint_aliases::ActiveModel {
                id: Set(Uuid::new_v4()),
                endpoint_id: Set(endpoint_id),
                target_kind: Set(target_kind.to_owned()),
                target_slug: Set(target_slug.to_owned()),
                alias: Set(alias.clone()),
            }
            .insert(conn)
            .await
            .map_err(db_err("endpoint::replace_aliases"))?;
        }
        Ok(())
    }

    async fn replace_auth_providers<C: ConnectionTrait>(
        &self,
        conn: &C,
        endpoint_id: Uuid,
        wanted: &BTreeSet<Slug>,
    ) -> Result<(), StoreError> {
        endpoint_auth_providers::Entity::delete_many()
            .filter(endpoint_auth_providers::Column::EndpointId.eq(endpoint_id))
            .exec(conn)
            .await
            .map_err(db_err("endpoint::replace_auth_providers"))?;
        let providers = AuthProviderStore::new(self.db.clone());
        for slug in wanted {
            let auth_provider_id = providers.id_by_slug_global(slug).await?;
            endpoint_auth_providers::ActiveModel {
                endpoint_id: Set(endpoint_id),
                auth_provider_id: Set(auth_provider_id),
            }
            .insert(conn)
            .await
            .map_err(db_err("endpoint::replace_auth_providers"))?;
        }
        Ok(())
    }
}

fn to_active_model(endpoint: &EndpointDef, id: Uuid) -> Result<endpoints::ActiveModel, StoreError> {
    Ok(endpoints::ActiveModel {
        id: Set(id),
        slug: Set(endpoint.slug.as_str().to_owned()),
        tag_expr: Set(tag_expr::to_string(&endpoint.tag_expr)),
        write_ceiling: Set(access_to_str(endpoint.write_ceiling).to_owned()),
        budgets: Set(budgets_to_json(&endpoint.budgets)),
        instructions: Set(endpoint.instructions.clone()),
        enabled: Set(endpoint.enabled),
        ..Default::default()
    })
}

fn to_model(
    row: endpoints::Model,
    aliases: BTreeMap<String, EndpointTarget>,
    auth_providers: BTreeSet<Slug>,
) -> Result<EndpointDef, StoreError> {
    Ok(EndpointDef {
        slug: parse_slug(&row.slug)?,
        tag_expr: tag_expr::parse(&row.tag_expr)
            .map_err(|e| StoreError::Malformed(e.to_string()))?,
        write_ceiling: str_to_access(&row.write_ceiling)?,
        budgets: json_to_budgets(&row.budgets)?,
        instructions: row.instructions,
        enabled: row.enabled,
        aliases,
        auth_providers,
    })
}

#[derive(Serialize, Deserialize, Default)]
struct BudgetsJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wall_clock_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_pages: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_concurrency: Option<u32>,
}

fn budgets_to_json(budgets: &Budgets) -> serde_json::Value {
    let json = BudgetsJson {
        max_calls: budgets.max_calls,
        max_bytes: budgets.max_bytes,
        wall_clock_ms: budgets.wall_clock.map(|d| d.as_millis() as u64),
        max_pages: budgets.max_pages,
        max_concurrency: budgets.max_concurrency,
    };
    serde_json::to_value(json).expect("BudgetsJson serialization is infallible for this shape")
}

fn json_to_budgets(value: &serde_json::Value) -> Result<Budgets, StoreError> {
    let parsed: BudgetsJson = serde_json::from_value(value.clone())
        .map_err(|e| StoreError::Malformed(format!("endpoints.budgets: {e}")))?;
    Ok(Budgets {
        max_calls: parsed.max_calls,
        max_bytes: parsed.max_bytes,
        wall_clock: parsed.wall_clock_ms.map(std::time::Duration::from_millis),
        max_pages: parsed.max_pages,
        max_concurrency: parsed.max_concurrency,
    })
}
