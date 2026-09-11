//! m0001 — identity scaffold: `users` (accounts for the server-rendered login — no admin/
//! non-admin distinction, see `entity::users`'s own doc), `sessions` (server-rendered login,
//! I5's "login and consent are server-rendered" design corner), `service_tokens` (long-lived
//! MCP bearer tokens) and `meta` (the `definitions_generation` counter the plan cache is keyed
//! on).
//!
//! Deviation from the plan's literal schema text: it gives `service_tokens` a `scopes` column
//! (`TEXT[]`, since `sea-orm`'s Postgres array support needs a `postgres-array` feature this
//! crate doesn't enable). This migration has none: `scopes` used to distinguish `"mcp"` from
//! `"admin"` tokens, and once the admin/non-admin distinction was removed (see
//! `server::identity`'s module doc) the column could only ever hold one value — which encodes
//! nothing, so it never existed here rather than being added and dropped in the same
//! unmerged branch. A resolved, unrevoked, unexpired token may call tools over `/mcp`; that's
//! the whole rule.
//!
//! `users.password_hash` is nullable and `oidc_issuer`/`oidc_subject` are new. A user typically
//! has exactly one of a local password or a linked external OIDC identity, but a third state is
//! also valid: both `NULL` — a pending account `api2mcp user add --oidc-only` pre-provisions,
//! claimed by `store::user::find_or_create_by_oidc` the first time a verified-email OIDC
//! sign-in matches it (`entity::users`'s own doc has the full reasoning). The `CHECK`
//! constraint below therefore only enforces that `oidc_issuer`/`oidc_subject` are set *together
//! or not at all* — it does not require a password or an identity to be present — and the
//! partial unique index on the OIDC pair still guarantees `(issuer, subject)` uniqueness
//! whenever both are set. This migration is amended in place rather than followed by a
//! corrective one — the branch is unmerged and nothing is deployed yet.
//!
//! A stricter `ck_users_has_a_login`-shaped constraint (`password_hash IS NOT NULL OR
//! oidc_issuer IS NOT NULL`) was considered and rejected: a `CHECK` only ever sees a row's own
//! columns, so it cannot tell "no login yet, deliberately pending" apart from "no login, and
//! never should have been possible" — the pending state needs the *first* to be allowed, and a
//! plain `CHECK` can't express that distinction without a new column (e.g. an explicit `status`)
//! recording which case a `NULL`-`NULL` row is in, which is out of scope here. The application
//! layer already enforces the invariant that matters: `create_pending_oidc` is the only writer
//! that can produce a `NULL`-`NULL` row at all, and `verify_password`/the `(issuer, subject)`
//! lookup both correctly refuse it a working login until it is claimed.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

use super::helpers::{timestamptz_now, timestamptz_null, uuid_col, uuid_pk};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0001_init"
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Email,
    PasswordHash,
    OidcIssuer,
    OidcSubject,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Sessions {
    Table,
    TokenHash,
    UserId,
    ExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ServiceTokens {
    Table,
    Id,
    TokenHash,
    TokenPrefix,
    OwnerId,
    Label,
    LastUsedAt,
    ExpiresAt,
    RevokedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Meta {
    Table,
    Key,
    Value,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(uuid_pk(Users::Id))
                    .col(ColumnDef::new(Users::Email).text().not_null())
                    .col(ColumnDef::new(Users::PasswordHash).text().null())
                    .col(ColumnDef::new(Users::OidcIssuer).text().null())
                    .col(ColumnDef::new(Users::OidcSubject).text().null())
                    .col(timestamptz_now(Users::CreatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_users_email")
                    .table(Users::Table)
                    .col(Users::Email)
                    .unique()
                    .to_owned(),
            )
            .await?;
        // `oidc_issuer`/`oidc_subject` are a pair: either both set (a linked identity) or both
        // `NULL` (a local-password account, or a pending `user add --oidc-only` row with no
        // login yet — see this file's module doc for why that third state is intentional). The
        // `sea_query` builder has no portable `CHECK` support, so this (and the partial unique
        // index below, which `sea_query::Index` cannot express either — it has no `WHERE`) are
        // raw SQL, same as m0007's seed does for the same reason.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE users ADD CONSTRAINT ck_users_oidc_pair CHECK ( \
                    (oidc_issuer IS NULL) = (oidc_subject IS NULL) \
                )",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX ux_users_oidc_identity ON users (oidc_issuer, oidc_subject) \
                 WHERE oidc_issuer IS NOT NULL AND oidc_subject IS NOT NULL",
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Sessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sessions::TokenHash)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(uuid_col(Sessions::UserId))
                    .col(
                        ColumnDef::new(Sessions::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(timestamptz_now(Sessions::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sessions_user")
                            .from(Sessions::Table, Sessions::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ServiceTokens::Table)
                    .if_not_exists()
                    .col(uuid_pk(ServiceTokens::Id))
                    .col(ColumnDef::new(ServiceTokens::TokenHash).text().not_null())
                    .col(ColumnDef::new(ServiceTokens::TokenPrefix).text().not_null())
                    .col(uuid_col(ServiceTokens::OwnerId))
                    .col(ColumnDef::new(ServiceTokens::Label).text().not_null())
                    .col(timestamptz_null(ServiceTokens::LastUsedAt))
                    .col(timestamptz_null(ServiceTokens::ExpiresAt))
                    .col(timestamptz_null(ServiceTokens::RevokedAt))
                    .col(timestamptz_now(ServiceTokens::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_tokens_owner")
                            .from(ServiceTokens::Table, ServiceTokens::OwnerId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_service_tokens_token_hash")
                    .table(ServiceTokens::Table)
                    .col(ServiceTokens::TokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Meta::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Meta::Key).text().not_null().primary_key())
                    .col(ColumnDef::new(Meta::Value).text().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Meta::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ServiceTokens::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Sessions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await?;
        Ok(())
    }
}
