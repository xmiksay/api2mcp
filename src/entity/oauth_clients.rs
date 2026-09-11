//! `oauth_clients` — dynamically registered OAuth 2.1 clients (RFC 7591). Public clients
//! (PKCE only, e.g. an MCP client doing DCR) have `client_secret_hash = NULL`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_clients")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub client_secret_hash: Option<String>,
    pub client_name: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub redirect_uris: Json,
    #[sea_orm(column_type = "JsonBinary")]
    pub grant_types: Json,
    pub token_endpoint_auth_method: String,
    pub scope: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
