//! `endpoint_aliases` — rename a tag-selected api_call or script as seen through one
//! endpoint, without touching the underlying definition's own slug.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "endpoint_aliases")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub endpoint_id: Uuid,
    /// `api_call` | `script`.
    pub target_kind: String,
    pub target_slug: String,
    pub alias: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::endpoints::Entity",
        from = "Column::EndpointId",
        to = "super::endpoints::Column::Id"
    )]
    Endpoint,
}

impl Related<super::endpoints::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Endpoint.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
