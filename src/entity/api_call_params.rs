//! `api_call_params` — one typed input to an `api_call`. `position` exists purely so
//! schema generation and fan-out have a stable iteration order (I7) — it is not a
//! priority or a display hint, and [`crate::schema::input_schema`]/[`crate::runtime::fanout`]
//! must iterate params by this column, never by name or insertion order.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "api_call_params")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub api_call_id: Uuid,
    pub name: String,
    /// `path` | `query` | `header` | `body`.
    pub location: String,
    /// `string` | `integer` | `number` | `boolean` | `array` | `object`.
    pub data_type: String,
    pub required: bool,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub default_value: Option<Json>,
    /// A definition-time constant injected on every call; mutually exclusive with
    /// `required` (`CHECK (fixed_value IS NULL OR required = false)`) — a fixed value can
    /// never simultaneously be something the caller is required to supply.
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub fixed_value: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub enum_values: Option<Json>,
    pub body_path: Option<String>,
    /// Stable schema-generation and fan-out order (I7) — see the module doc.
    pub position: i32,
    /// What `schema::input_schema` emits as the property's `description`. `NOT NULL
    /// DEFAULT ''` — "no description" and "empty description" are the same thing to a
    /// schema consumer, so the store layer collapses `''` to `None` rather than carrying a
    /// third state through `model::Param::description`.
    pub description: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::api_calls::Entity",
        from = "Column::ApiCallId",
        to = "super::api_calls::Column::Id"
    )]
    ApiCall,
}

impl Related<super::api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCall.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
