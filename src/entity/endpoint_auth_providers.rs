//! `endpoint_auth_providers` — I5's human-set join: which auth providers an endpoint is
//! permitted to bind to. No MCP tool or write route touches this table; only
//! `pack/import.rs` and the `cli` write it (`store::auth_provider`'s write methods are
//! `pub(crate)` with exactly those two callers).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "endpoint_auth_providers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub endpoint_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub auth_provider_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::endpoints::Entity",
        from = "Column::EndpointId",
        to = "super::endpoints::Column::Id"
    )]
    Endpoint,
    #[sea_orm(
        belongs_to = "super::auth_providers::Entity",
        from = "Column::AuthProviderId",
        to = "super::auth_providers::Column::Id"
    )]
    AuthProvider,
}

impl Related<super::endpoints::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Endpoint.def()
    }
}

impl Related<super::auth_providers::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuthProvider.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
