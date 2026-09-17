//! Hand-written SeaORM entities, one module per table. `sea_orm` is a leaf dependency of
//! this module (plus [`crate::migration`] and [`crate::store`]) — nothing above `store`
//! ever sees `entity::Model` directly.
//!
//! Every `JSONB` column — the plan's `TEXT[]` array columns included — is typed as
//! [`sea_orm::entity::prelude::Json`] (`serde_json::Value`) here. `Vec<String>` would be
//! the more precise type for a column like `origin_allowlist`, but `sea-orm`'s Postgres
//! array (de)serialization needs the `postgres-array` feature, which `Cargo.toml`
//! does not enable; `sea_query::Value::Json` is hard-coded to
//! `serde_json::Value` regardless. Entities stay dumb row mirrors — [`crate::store`]
//! converts into the typed [`crate::model`] shapes the rest of the crate sees.
//!
//! Relations are declared wherever the schema has a real foreign key (a `belongs_to` on
//! the owning side, with a reciprocal `has_many` on tables the plan groups as one
//! aggregate — `services`/`auth_providers`/`api_calls`/`api_call_params`,
//! `scripts`/`script_params`/`script_api_calls`, `endpoints`/`endpoint_aliases`,
//! `runs`/`run_calls`). Pure lookup and join tables (sessions, tokens, oauth bookkeeping,
//! tag joins) only declare the `belongs_to` side; nothing yet needs to eager-load from
//! their parent, and adding it speculatively would be scaffolding ahead of a caller.

pub mod api_call_params;
pub mod api_call_tags;
pub mod api_calls;
pub mod auth_providers;
pub mod endpoint_aliases;
pub mod endpoint_auth_providers;
pub mod endpoints;
pub mod meta;
pub mod oauth_clients;
pub mod oauth_codes;
pub mod oauth_consent_requests;
pub mod oauth_consents;
pub mod oauth_tokens;
pub mod run_calls;
pub mod runs;
pub mod script_api_calls;
pub mod script_params;
pub mod script_tags;
pub mod scripts;
pub mod service_token_endpoints;
pub mod service_tokens;
pub mod services;
pub mod sessions;
pub mod tags;
pub mod users;
