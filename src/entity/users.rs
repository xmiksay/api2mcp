//! `users` — accounts for the server-rendered login (no `LoginView.vue`; see `plan.md`'s risk
//! 2). There is no admin/non-admin distinction: every user can read and write every definition,
//! so this row is purely an identity, not a role.
//!
//! A user typically has exactly one of two logins: `password_hash` (argon2id PHC string, local
//! password) or `(oidc_issuer, oidc_subject)` (a linked external identity, `server::oidc`). A
//! third, deliberately valid state exists too: a row with both `password_hash` and the OIDC
//! pair `NULL` — an account pre-provisioned by `api2mcp user add --oidc-only`
//! (`store::user::create_pending_oidc`) that has no working login yet, pending a linking step a
//! follow-up chunk adds. `ck_users_oidc_pair` (`migration::m0001_init`) only enforces that
//! `oidc_issuer`/`oidc_subject` are set *together or not at all*; `ux_users_oidc_identity` (a
//! partial unique index, same migration) enforces the pair is unique whenever both are set.
//! Matching an OIDC sign-in is always by `(issuer, subject)`, never by `email`: a provider is
//! free to let a user change their email, and matching on it would let a changed email hijack
//! another account (`store::user::find_or_create_by_oidc`).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub email: String,
    pub password_hash: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_subject: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
