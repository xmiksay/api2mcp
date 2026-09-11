//! `tags` façade — the vocabulary `endpoint.tag_expr` selects api_calls/scripts over, plus
//! the `api_call_tags`/`script_tags` membership joins. Membership-setting is
//! `pub(crate)`: it's an internal helper `store::api_call`/`store::script` use while
//! writing an aggregate, not something callers reach directly (a tag write always happens
//! as part of writing the api_call/script that carries it).

use std::collections::BTreeSet;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};
use uuid::Uuid;

use crate::entity::{api_call_tags, script_tags, tags};
use crate::model::Tag;

use super::{StoreError, db_err, parse_slug};

#[derive(Clone)]
pub struct TagStore {
    db: DatabaseConnection,
}

impl TagStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn list(&self) -> Result<BTreeSet<Tag>, StoreError> {
        let rows = tags::Entity::find()
            .all(&self.db)
            .await
            .map_err(db_err("tag::list"))?;
        rows.into_iter()
            .map(|r| parse_slug(&r.name).map(Tag))
            .collect()
    }

    pub(crate) async fn tags_for_api_call(
        &self,
        api_call_id: Uuid,
    ) -> Result<BTreeSet<Tag>, StoreError> {
        let ids: Vec<Uuid> = api_call_tags::Entity::find()
            .filter(api_call_tags::Column::ApiCallId.eq(api_call_id))
            .all(&self.db)
            .await
            .map_err(db_err("tag::tags_for_api_call"))?
            .into_iter()
            .map(|r| r.tag_id)
            .collect();
        self.tags_by_ids(&ids).await
    }

    pub(crate) async fn tags_for_script(
        &self,
        script_id: Uuid,
    ) -> Result<BTreeSet<Tag>, StoreError> {
        let ids: Vec<Uuid> = script_tags::Entity::find()
            .filter(script_tags::Column::ScriptId.eq(script_id))
            .all(&self.db)
            .await
            .map_err(db_err("tag::tags_for_script"))?
            .into_iter()
            .map(|r| r.tag_id)
            .collect();
        self.tags_by_ids(&ids).await
    }

    /// Replaces every tag membership row for `api_call_id` with `wanted`, creating any tag
    /// that doesn't exist yet. Takes the caller's connection (generic over
    /// [`ConnectionTrait`], matching `EndpointStore::replace_aliases`/`replace_auth_providers`
    /// and `ScriptStore::replace_callable`/`replace_params`) rather than opening its own
    /// transaction: `ApiCallStore::create`/`update` call this while their own row insert is
    /// still uncommitted on a separate transaction, so a second, independent transaction here
    /// couldn't see that row yet and every tag insert would trip the `api_call_tags` foreign
    /// key. Composing into the caller's transaction is what makes the api_call row and its
    /// tags commit — or fail — atomically, and is also what lets a reader never observe a
    /// half-updated tag set.
    pub(crate) async fn set_api_call_tags<C: ConnectionTrait>(
        &self,
        conn: &C,
        api_call_id: Uuid,
        wanted: &BTreeSet<Tag>,
    ) -> Result<(), StoreError> {
        api_call_tags::Entity::delete_many()
            .filter(api_call_tags::Column::ApiCallId.eq(api_call_id))
            .exec(conn)
            .await
            .map_err(db_err("tag::set_api_call_tags"))?;
        for tag in wanted {
            let tag_id = self.ensure(conn, tag).await?;
            api_call_tags::ActiveModel {
                api_call_id: Set(api_call_id),
                tag_id: Set(tag_id),
            }
            .insert(conn)
            .await
            .map_err(db_err("tag::set_api_call_tags"))?;
        }
        Ok(())
    }

    /// See [`Self::set_api_call_tags`] for why this takes the caller's connection instead of
    /// opening its own transaction.
    pub(crate) async fn set_script_tags<C: ConnectionTrait>(
        &self,
        conn: &C,
        script_id: Uuid,
        wanted: &BTreeSet<Tag>,
    ) -> Result<(), StoreError> {
        script_tags::Entity::delete_many()
            .filter(script_tags::Column::ScriptId.eq(script_id))
            .exec(conn)
            .await
            .map_err(db_err("tag::set_script_tags"))?;
        for tag in wanted {
            let tag_id = self.ensure(conn, tag).await?;
            script_tags::ActiveModel {
                script_id: Set(script_id),
                tag_id: Set(tag_id),
            }
            .insert(conn)
            .await
            .map_err(db_err("tag::set_script_tags"))?;
        }
        Ok(())
    }

    async fn tags_by_ids(&self, ids: &[Uuid]) -> Result<BTreeSet<Tag>, StoreError> {
        if ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        let rows = tags::Entity::find()
            .filter(tags::Column::Id.is_in(ids.to_vec()))
            .all(&self.db)
            .await
            .map_err(db_err("tag::tags_by_ids"))?;
        rows.into_iter()
            .map(|r| parse_slug(&r.name).map(Tag))
            .collect()
    }

    /// Gets-or-creates a tag row for `tag.0`'s name, returning its primary key. Takes a
    /// generic connection so it can run inside the caller's transaction.
    async fn ensure<C: ConnectionTrait>(&self, conn: &C, tag: &Tag) -> Result<Uuid, StoreError> {
        if let Some(row) = tags::Entity::find()
            .filter(tags::Column::Name.eq(tag.0.as_str()))
            .one(conn)
            .await
            .map_err(db_err("tag::ensure"))?
        {
            return Ok(row.id);
        }
        let id = Uuid::new_v4();
        let active = tags::ActiveModel {
            id: Set(id),
            name: Set(tag.0.as_str().to_owned()),
            ..Default::default()
        };
        // A concurrent insert of the same name would trip the unique index; treat that race
        // as "someone else just created it" and look the row up again rather than surface a
        // raw constraint violation.
        if active.insert(conn).await.is_ok() {
            return Ok(id);
        }
        tags::Entity::find()
            .filter(tags::Column::Name.eq(tag.0.as_str()))
            .one(conn)
            .await
            .map_err(db_err("tag::ensure"))?
            .map(|r| r.id)
            .ok_or(StoreError::Db)
    }
}
