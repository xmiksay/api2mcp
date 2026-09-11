//! m0001 — identity scaffold: `users` (admin accounts), `sessions` (server-rendered
//! login, I5's "login and consent are server-rendered" design corner), `service_tokens`
//! (long-lived MCP bearer tokens) and `meta` (the `definitions_generation` counter the
//! plan cache is keyed on).
//!
//! Deviation from the plan's literal schema text: `service_tokens.scopes` is `TEXT[]`
//! there. `sea-orm`'s Postgres array (de)serialization is gated behind the
//! `postgres-array` feature, which is not enabled in `Cargo.toml` (out of this chunk's
//! file ownership) — a native array column would panic decoding rows. Stored as
//! `JSONB` holding a JSON array of strings instead; same two values (`mcp`, `admin`)
//! this iteration, same `Vec<String>` shape at the entity boundary.

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
    IsAdmin,
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
    Scopes,
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
                    .col(ColumnDef::new(Users::PasswordHash).text().not_null())
                    .col(
                        ColumnDef::new(Users::IsAdmin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
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
                    .col(
                        ColumnDef::new(ServiceTokens::Scopes)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
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
