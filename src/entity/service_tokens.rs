//! `service_tokens` — long-lived MCP bearer tokens. `token_hash` is a sha256 hex digest;
//! `token_prefix` is a short display fragment (e.g. the first 8 chars) so an operator can
//! recognize a token in a list without the plaintext ever having been stored. `scopes`
//! holds exactly `"mcp"`/`"admin"` this iteration (see [`crate::entity::mod`] on why it's
//! `Json` and not `Vec<String>`).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "service_tokens")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub token_hash: String,
    pub token_prefix: String,
    pub owner_id: Uuid,
    pub label: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub scopes: Json,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    pub expires_at: Option<DateTimeWithTimeZone>,
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::OwnerId",
        to = "super::users::Column::Id"
    )]
    Owner,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Owner.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
