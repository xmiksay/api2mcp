//! m0002 — the OAuth 2.1 authorization server: dynamically registered `oauth_clients`,
//! single-use `oauth_codes` (PKCE), `oauth_tokens` (hashed access/refresh pair with a
//! `family_id` for refresh-reuse-revokes-the-family detection), standing `oauth_consents`
//! and the `oauth_consent_requests` the server-rendered consent screen round-trips
//! through `/oauth/authorize`.
//!
//! Every token here is stored as a **sha256 hash**, never plaintext (`access_token_hash`,
//! `refresh_token_hash`, `oauth_codes.code_hash`) — see [`super::m0001_init`]'s note on
//! `service_tokens`; the same "a token is 244 bits of randomness, a fast hash is enough"
//! reasoning applies. `oauth_clients.redirect_uris`/`grant_types` and `oauth_codes`/
//! `oauth_consents`/`oauth_consent_requests`' `scope` lists are `JSONB` for the same
//! `postgres-array`-feature reason documented in m0001.
//!
//! `Users { Table, Id }` is redeclared locally rather than imported from
//! [`super::m0001_init`]: migrations are append-only and never edited again once shipped,
//! so each file stays independently readable instead of coupling to another migration's
//! private identifiers.

use sea_orm_migration::prelude::*;

use super::helpers::{timestamptz_now, uuid_col, uuid_pk};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0002_oauth"
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum OauthClients {
    Table,
    Id,
    ClientSecretHash,
    ClientName,
    RedirectUris,
    GrantTypes,
    TokenEndpointAuthMethod,
    Scope,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthCodes {
    Table,
    CodeHash,
    ClientId,
    UserId,
    RedirectUri,
    CodeChallenge,
    CodeChallengeMethod,
    Resource,
    Scope,
    Used,
    ExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthTokens {
    Table,
    Id,
    AccessTokenHash,
    RefreshTokenHash,
    ClientId,
    UserId,
    FamilyId,
    Scope,
    Resource,
    Revoked,
    AccessExpiresAt,
    RefreshExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthConsents {
    Table,
    Id,
    UserId,
    ClientId,
    Scope,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthConsentRequests {
    Table,
    Id,
    ClientId,
    RedirectUri,
    Scope,
    Resource,
    CodeChallenge,
    CodeChallengeMethod,
    State,
    ExpiresAt,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OauthClients::Table)
                    .if_not_exists()
                    .col(uuid_pk(OauthClients::Id))
                    .col(ColumnDef::new(OauthClients::ClientSecretHash).text().null())
                    .col(ColumnDef::new(OauthClients::ClientName).text().not_null())
                    .col(
                        ColumnDef::new(OauthClients::RedirectUris)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthClients::GrantTypes)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust(
                                "'[\"authorization_code\",\"refresh_token\"]'::jsonb",
                            )),
                    )
                    .col(
                        ColumnDef::new(OauthClients::TokenEndpointAuthMethod)
                            .text()
                            .not_null()
                            .default("none"),
                    )
                    .col(ColumnDef::new(OauthClients::Scope).text().null())
                    .col(timestamptz_now(OauthClients::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthCodes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthCodes::CodeHash)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(uuid_col(OauthCodes::ClientId))
                    .col(uuid_col(OauthCodes::UserId))
                    .col(ColumnDef::new(OauthCodes::RedirectUri).text().not_null())
                    .col(ColumnDef::new(OauthCodes::CodeChallenge).text().not_null())
                    .col(
                        ColumnDef::new(OauthCodes::CodeChallengeMethod)
                            .text()
                            .not_null()
                            .default("S256"),
                    )
                    .col(ColumnDef::new(OauthCodes::Resource).text().null())
                    .col(ColumnDef::new(OauthCodes::Scope).text().null())
                    .col(
                        ColumnDef::new(OauthCodes::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(OauthCodes::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(timestamptz_now(OauthCodes::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_codes_client")
                            .from(OauthCodes::Table, OauthCodes::ClientId)
                            .to(OauthClients::Table, OauthClients::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_codes_user")
                            .from(OauthCodes::Table, OauthCodes::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthTokens::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::AccessTokenHash)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthTokens::RefreshTokenHash).text().null())
                    .col(uuid_col(OauthTokens::ClientId))
                    .col(uuid_col(OauthTokens::UserId))
                    .col(uuid_col(OauthTokens::FamilyId))
                    .col(ColumnDef::new(OauthTokens::Scope).text().null())
                    .col(ColumnDef::new(OauthTokens::Resource).text().null())
                    .col(
                        ColumnDef::new(OauthTokens::Revoked)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::AccessExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::RefreshExpiresAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(timestamptz_now(OauthTokens::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_tokens_client")
                            .from(OauthTokens::Table, OauthTokens::ClientId)
                            .to(OauthClients::Table, OauthClients::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_tokens_user")
                            .from(OauthTokens::Table, OauthTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_oauth_tokens_access_hash")
                    .table(OauthTokens::Table)
                    .col(OauthTokens::AccessTokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_oauth_tokens_refresh_hash")
                    .table(OauthTokens::Table)
                    .col(OauthTokens::RefreshTokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthConsents::Table)
                    .if_not_exists()
                    .col(uuid_pk(OauthConsents::Id))
                    .col(uuid_col(OauthConsents::UserId))
                    .col(uuid_col(OauthConsents::ClientId))
                    .col(ColumnDef::new(OauthConsents::Scope).text().not_null())
                    .col(timestamptz_now(OauthConsents::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_consents_user")
                            .from(OauthConsents::Table, OauthConsents::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_consents_client")
                            .from(OauthConsents::Table, OauthConsents::ClientId)
                            .to(OauthClients::Table, OauthClients::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_oauth_consents_user_client")
                    .table(OauthConsents::Table)
                    .col(OauthConsents::UserId)
                    .col(OauthConsents::ClientId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthConsentRequests::Table)
                    .if_not_exists()
                    .col(uuid_pk(OauthConsentRequests::Id))
                    .col(uuid_col(OauthConsentRequests::ClientId))
                    .col(
                        ColumnDef::new(OauthConsentRequests::RedirectUri)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthConsentRequests::Scope).text().null())
                    .col(ColumnDef::new(OauthConsentRequests::Resource).text().null())
                    .col(
                        ColumnDef::new(OauthConsentRequests::CodeChallenge)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::CodeChallengeMethod)
                            .text()
                            .not_null()
                            .default("S256"),
                    )
                    .col(ColumnDef::new(OauthConsentRequests::State).text().null())
                    .col(
                        ColumnDef::new(OauthConsentRequests::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(timestamptz_now(OauthConsentRequests::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_consent_requests_client")
                            .from(OauthConsentRequests::Table, OauthConsentRequests::ClientId)
                            .to(OauthClients::Table, OauthClients::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthConsentRequests::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthConsents::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthTokens::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthCodes::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthClients::Table).to_owned())
            .await?;
        Ok(())
    }
}
