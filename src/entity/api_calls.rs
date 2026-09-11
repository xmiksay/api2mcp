//! `api_calls` — one HTTP call definition: method, path template (I3), fixed query/body,
//! read/write `access`, response projection and pagination.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "api_calls")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub service_id: Uuid,
    pub auth_provider_id: Option<Uuid>,
    pub slug: String,
    /// `GET` | `POST` | `PUT` | `PATCH` | `DELETE`.
    pub method: String,
    pub path_template: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub query_fixed: Json,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub body_template: Option<Json>,
    /// `read` | `write` — the ceiling an endpoint's `write_ceiling` is checked against.
    pub access: String,
    pub idempotent: bool,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub projection: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub pagination: Option<Json>,
    pub timeout_ms: Option<i32>,
    pub max_response_bytes: Option<i64>,
    /// What `server::mcp::registry::describe` uses as the MCP tool's `description` when
    /// present. `NOT NULL DEFAULT ''` — see `api_call_params.description` for why the store
    /// layer, not this column, is where `''` collapses to `None`.
    pub description: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::services::Entity",
        from = "Column::ServiceId",
        to = "super::services::Column::Id"
    )]
    Service,
    #[sea_orm(
        belongs_to = "super::auth_providers::Entity",
        from = "Column::AuthProviderId",
        to = "super::auth_providers::Column::Id"
    )]
    AuthProvider,
    #[sea_orm(has_many = "super::api_call_params::Entity")]
    Params,
}

impl Related<super::services::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Service.def()
    }
}

impl Related<super::auth_providers::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuthProvider.def()
    }
}

impl Related<super::api_call_params::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Params.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
