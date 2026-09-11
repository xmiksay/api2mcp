//! `meta` façade — a generic key/value table whose one load-bearing row is
//! `definitions_generation`, the counter [`crate::resolve`]'s plan cache (chunk C6) is keyed
//! on. Every write path in this module that changes a definition (a service, auth
//! provider, api_call, script, endpoint, or tag membership) bumps this counter in the same
//! transaction as its own write, so a cached `EndpointPlan` invalidates the moment anything
//! it was built from changes.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};

use crate::entity::meta;

use super::{StoreError, db_err};

pub const DEFINITIONS_GENERATION_KEY: &str = "definitions_generation";

#[derive(Clone)]
pub struct MetaStore {
    db: DatabaseConnection,
}

impl MetaStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// The current generation. Absent row (a fresh, unmigrated-by-any-write database) reads
    /// as `0`, so the first bump always produces `1`.
    pub async fn definitions_generation(&self) -> Result<u64, StoreError> {
        Self::read_generation(&self.db).await
    }

    /// Bumps the counter by 1 over this store's own connection, for a caller that isn't
    /// already inside a transaction.
    pub async fn bump_definitions_generation(&self) -> Result<u64, StoreError> {
        Self::bump_generation_in(&self.db).await
    }

    /// Reads the counter over any connection, including a caller's open transaction.
    pub(crate) async fn read_generation<C: ConnectionTrait>(conn: &C) -> Result<u64, StoreError> {
        let row = meta::Entity::find()
            .filter(meta::Column::Key.eq(DEFINITIONS_GENERATION_KEY))
            .one(conn)
            .await
            .map_err(db_err("meta::read_generation"))?;
        match row {
            Some(r) => r
                .value
                .parse::<u64>()
                .map_err(|e| StoreError::Malformed(format!("meta.definitions_generation: {e}"))),
            None => Ok(0),
        }
    }

    /// Bumps the counter by 1 over a caller-supplied connection, so a definition write and
    /// its generation bump commit atomically as one transaction.
    pub(crate) async fn bump_generation_in<C: ConnectionTrait>(
        conn: &C,
    ) -> Result<u64, StoreError> {
        let next = Self::read_generation(conn).await? + 1;
        let exists = meta::Entity::find()
            .filter(meta::Column::Key.eq(DEFINITIONS_GENERATION_KEY))
            .one(conn)
            .await
            .map_err(db_err("meta::bump_generation_in"))?
            .is_some();
        let active = meta::ActiveModel {
            key: Set(DEFINITIONS_GENERATION_KEY.to_owned()),
            value: Set(next.to_string()),
        };
        if exists {
            active
                .update(conn)
                .await
                .map_err(db_err("meta::bump_generation_in"))?;
        } else {
            active
                .insert(conn)
                .await
                .map_err(db_err("meta::bump_generation_in"))?;
        }
        Ok(next)
    }
}
