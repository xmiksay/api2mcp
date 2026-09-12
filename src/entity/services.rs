//! `services` — one upstream: its base URL, the origins its api_calls may reach (I2),
//! default headers, and the limits an [`crate::runtime::budget`] meter folds against.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "services")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub owner_id: Uuid,
    pub slug: String,
    pub base_url: String,
    /// `base_url`'s own origin must be a member of this list (checked at publish time,
    /// not here — see the plan's design correction 7).
    #[sea_orm(column_type = "JsonBinary")]
    pub origin_allowlist: Json,
    #[sea_orm(column_type = "JsonBinary")]
    pub default_headers: Json,
    pub timeout_ms: i32,
    pub max_concurrency: i32,
    pub rate_limit_per_min: i32,
    pub max_response_bytes: i64,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::auth_providers::Entity")]
    AuthProviders,
    #[sea_orm(has_many = "super::api_calls::Entity")]
    ApiCalls,
}

impl Related<super::auth_providers::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuthProviders.def()
    }
}

impl Related<super::api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCalls.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
