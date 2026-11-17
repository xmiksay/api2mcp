//! `endpoint_auth_providers` — I5's human-set join: which auth providers an endpoint is
//! permitted to bind to. Written only through `store::endpoint`, by a human: the
//! `/api/endpoints` routes or a pack import (`api2mcp import`, `POST /api/packs/import`). The
//! control-plane MCP `endpoint.*` tools cannot choose it — they pin it empty on create and
//! preserve the existing set on update.

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
