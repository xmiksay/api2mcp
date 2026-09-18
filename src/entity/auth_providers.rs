//! `auth_providers` — a named credential binding. Exactly one of two columns names where the
//! credential comes from: `credential_env_key`, the *name* of an `A2M_CRED_*` environment
//! variable that [`crate::secret`] resolves at runtime, or `credential_value`, the credential
//! itself, held in plaintext for the per-owner case an environment variable cannot express. See
//! [`crate::model::CredentialSource`] for why that trade is made and why I4 still holds.
//!
//! `bound_origin` is I5: a human sets which origin this credential may be used against, and
//! `resolve::auth_bind` asserts every api_call on this provider's own service actually resolves
//! to that origin.
//!
//! `service_id` is `UNIQUE` (`ux_auth_providers_service`): a service has at most one auth
//! provider, and every api_call on that service uses it — an api_call names no provider of its
//! own at all (see [`crate::model::ApiCall`]).

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
    /// Name of an `A2M_CRED_*` env var. `NULL` iff this provider stores its value below.
    pub credential_env_key: Option<String>,
    /// The credential itself, in plaintext, for a provider whose value is not in the
    /// environment. `NULL` both for an env-backed provider and for a stored-source provider
    /// whose owner has not set a value yet.
    pub credential_value: Option<String>,
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
}

impl Related<super::services::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Service.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
