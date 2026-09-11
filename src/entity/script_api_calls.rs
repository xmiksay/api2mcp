//! `script_api_calls` — I1's *declarative* half: the fixed, human-curated set of
//! api_calls a script may invoke, each under an `alias` the script's `api()`/`api_many()`
//! calls use as the callee name. `runtime::dispatch::ApiDispatcher::dispatch` resolves
//! every name through this join (intersected with the endpoint's selection); nothing
//! outside that resolution path can reach `http::send`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "script_api_calls")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub script_id: Uuid,
    pub api_call_id: Uuid,
    pub alias: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::scripts::Entity",
        from = "Column::ScriptId",
        to = "super::scripts::Column::Id"
    )]
    Script,
    #[sea_orm(
        belongs_to = "super::api_calls::Entity",
        from = "Column::ApiCallId",
        to = "super::api_calls::Column::Id"
    )]
    ApiCall,
}

impl Related<super::scripts::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Script.def()
    }
}

impl Related<super::api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCall.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
