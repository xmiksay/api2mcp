//! `runs` — the audit record of one tool execution. `definition_snapshot` holds the full
//! compiled slice (api_call/script plus its params, projection, the service row minus
//! credentials, and the folded budgets) — with I8 dropped, this is the only answer to
//! "what did this tool look like when it ran", so it must be complete. `definition_digest`
//! is the sha256 of that snapshot's canonical JSON, compared byte-for-byte by the pack
//! export/import round-trip test.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub endpoint_slug: String,
    pub tool_name: String,
    /// `api_call` | `script`.
    pub target_kind: String,
    pub target_slug: String,
    /// `oauth` | `service_token`.
    pub caller_kind: String,
    pub caller_id: String,
    pub request_id: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub definition_snapshot: Json,
    pub definition_digest: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub input_redacted: Json,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub output_redacted: Option<Json>,
    /// `ok` | `partial` | `error` | `denied` | `budget_exceeded` | `timeout`.
    pub status: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub errors: Option<Json>,
    pub calls_made: i32,
    pub bytes_in: i64,
    pub pages_fetched: i32,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub budget_snapshot: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub timings: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::run_calls::Entity")]
    Calls,
}

impl Related<super::run_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Calls.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
