//! `oauth_tokens` — hashed access/refresh token pairs. `family_id` groups every token
//! descended from one authorization: refresh rotation replaces a row's refresh hash
//! within the family, and reusing a revoked refresh token revokes the whole family (theft
//! detection per RFC 9700 6.1's rotation-and-family recommendation).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_tokens")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub access_token_hash: String,
    pub refresh_token_hash: Option<String>,
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub scope: Option<String>,
    pub resource: Option<String>,
    pub revoked: bool,
    pub access_expires_at: DateTimeWithTimeZone,
    pub refresh_expires_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::oauth_clients::Entity",
        from = "Column::ClientId",
        to = "super::oauth_clients::Column::Id"
    )]
    Client,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id"
    )]
    User,
}

impl Related<super::oauth_clients::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Client.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
