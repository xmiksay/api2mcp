//! `service_tokens` — long-lived MCP bearer tokens. `token_hash` is a sha256 hex digest;
//! `token_prefix` is a short display fragment (e.g. the first 8 chars) so an operator can
//! recognize a token in a list without the plaintext ever having been stored.
//!
//! No `scopes` column: it used to hold `"mcp"` or `"admin"`, but the admin/non-admin
//! distinction is gone (see `server::identity`'s module doc), leaving exactly one possible
//! value — a column that can only ever hold one value encodes nothing, so it was dropped
//! entirely (`migration::m0001_init`) rather than kept as a decoration. A resolved,
//! unrevoked, unexpired token may call tools over `/mcp`; that's the whole rule now.

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
    /// Whether the token is confined to [`super::service_token_endpoints`]. Distinct from
    /// "has no grant rows": deleting the last granted endpoint cascades those rows away, and a
    /// restricted token must not widen into an unrestricted one as a result.
    pub restricted: bool,
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
