//! `run_calls` — one upstream HTTP request made during a run. `seq` is the **input index
//! of the fan-out batch, never its completion index** (I7 made durable: completion order
//! is never observable, so it must never be recorded as if it were meaningful order).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run_calls")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub run_id: Uuid,
    /// Input index of this call within its fan-out batch — see the module doc.
    pub seq: i32,
    pub api_call_slug: String,
    pub service_slug: String,
    pub method: String,
    pub url_redacted: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub headers_redacted: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub body_redacted: Option<Json>,
    pub status_code: Option<i32>,
    pub response_bytes: Option<i64>,
    pub response_truncated: bool,
    pub error: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub timings: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::runs::Entity",
        from = "Column::RunId",
        to = "super::runs::Column::Id"
    )]
    Run,
}

impl Related<super::runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
