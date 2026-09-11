//! `scripts` — a Rhai program exposed as a tool. `source` is the Rhai text;
//! `script_api_calls` (I1's declarative half) is the only way its `api()`/`api_many()`
//! calls can reach HTTP.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "scripts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub slug: String,
    pub description: Option<String>,
    pub source: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub projection: Option<Json>,
    pub timeout_ms: Option<i32>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::script_params::Entity")]
    Params,
    #[sea_orm(has_many = "super::script_api_calls::Entity")]
    ApiCalls,
}

impl Related<super::script_params::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Params.def()
    }
}

impl Related<super::script_api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCalls.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
