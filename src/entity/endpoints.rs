//! `endpoints` — a tag-expression selection of tools exposed at `POST /mcp/{slug}` (or the
//! bare `POST /mcp`, for `cfg.default_endpoint`). `write_ceiling` bounds the `access` level
//! of any api_call the tag expression selects; `budgets` is folded element-wise (`min`)
//! against every selected api_call's own budget before an [`crate::runtime::budget`]
//! meter is built.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "endpoints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub slug: String,
    pub tag_expr: String,
    /// `read` | `write`.
    pub write_ceiling: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub budgets: Json,
    /// Free-text MCP server instructions returned from `initialize` for this endpoint.
    pub instructions: Option<String>,
    pub enabled: bool,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::endpoint_aliases::Entity")]
    Aliases,
}

impl Related<super::endpoint_aliases::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Aliases.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
