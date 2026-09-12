//! `service_token_endpoints` — a self-service token's endpoint grant list. No rows for a
//! token means unrestricted (every endpoint, the permissive GitHub-classic-PAT-style
//! default); a non-empty row set means exactly those endpoints. Both foreign keys cascade
//! (`migration::m0005_endpoints`): deleting a token drops its grants, and deleting an
//! endpoint drops any grant that named it rather than leaving one dangling.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "service_token_endpoints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub service_token_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub endpoint_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::service_tokens::Entity",
        from = "Column::ServiceTokenId",
        to = "super::service_tokens::Column::Id"
    )]
    ServiceToken,
    #[sea_orm(
        belongs_to = "super::endpoints::Entity",
        from = "Column::EndpointId",
        to = "super::endpoints::Column::Id"
    )]
    Endpoint,
}

impl Related<super::service_tokens::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ServiceToken.def()
    }
}

impl Related<super::endpoints::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Endpoint.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
