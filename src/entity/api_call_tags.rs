//! `api_call_tags` — many-to-many membership of an `api_call` in a `tag`. Composite
//! primary key `(api_call_id, tag_id)`; no synthetic id, this is a pure join.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "api_call_tags")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub api_call_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub tag_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::api_calls::Entity",
        from = "Column::ApiCallId",
        to = "super::api_calls::Column::Id"
    )]
    ApiCall,
    #[sea_orm(
        belongs_to = "super::tags::Entity",
        from = "Column::TagId",
        to = "super::tags::Column::Id"
    )]
    Tag,
}

impl Related<super::api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCall.def()
    }
}

impl Related<super::tags::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
