//! `auth_providers` — a named credential binding. **No column here, or anywhere else in
//! the schema, ever holds a credential value** (I4's structural half) — `credential_env_key`
//! is the *name* of an `A2M_CRED_*` environment variable that [`crate::secret`] resolves
//! at runtime. `bound_origin` is I5: a human sets which origin this credential may be used
//! against, and `resolve::auth_bind` asserts every api_call using this provider actually
//! resolves to that origin.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "auth_providers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub owner_id: Uuid,
    pub service_id: Uuid,
    pub slug: String,
    /// `header` | `bearer` | `oauth2_client_credentials`.
    pub kind: String,
    /// Name of an `A2M_CRED_*` env var — never a credential value itself.
    pub credential_env_key: String,
    pub header_name: Option<String>,
    pub value_template: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub scopes: Option<Json>,
    pub token_url: Option<String>,
    pub bound_origin: String,
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
    #[sea_orm(has_many = "super::api_calls::Entity")]
    ApiCalls,
}

impl Related<super::services::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Service.def()
    }
}

impl Related<super::api_calls::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiCalls.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
