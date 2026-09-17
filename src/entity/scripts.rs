//! `scripts` — a Rhai program exposed as a tool. `source` is the Rhai text;
//! `script_api_calls` (I1's declarative half) is the only way its `api()`/`api_many()`
//! calls can reach HTTP. No `projection` column: a script returns its own
//! already-composed value, so there is nothing for a declarative JSONPath projection to
//! act on — that is strictly an `api_call` concept.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "scripts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub owner_id: Uuid,
    pub slug: String,
    pub description: Option<String>,
    pub source: String,
    /// A script's own budget opinion (I6) — see `model::budget::Budgets` for why `None`
    /// means "no opinion on this axis", never "unlimited", and why that asymmetry is what
    /// keeps `Budgets::fold` narrowing-only.
    pub max_calls: Option<i32>,
    pub max_bytes: Option<i64>,
    pub wall_clock_ms: Option<i32>,
    pub max_pages: Option<i32>,
    pub max_concurrency: Option<i32>,
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
