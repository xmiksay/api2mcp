//! `script_params` — a script's typed inputs. Same `data_type`/`required`/`default_value`/
//! `enum_values`/`position` shape as `api_call_params`, minus `location`/`fixed_value`/
//! `body_path`: those describe where a value lands in an HTTP request, which doesn't apply
//! to a script input bound as a plain Rhai variable.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "script_params")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub script_id: Uuid,
    pub name: String,
    /// `string` | `integer` | `number` | `boolean` | `array` | `object`.
    pub data_type: String,
    pub required: bool,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub default_value: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub enum_values: Option<Json>,
    /// Stable schema-generation order (I7), same role as `api_call_params.position`.
    pub position: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::scripts::Entity",
        from = "Column::ScriptId",
        to = "super::scripts::Column::Id"
    )]
    Script,
}

impl Related<super::scripts::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Script.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
