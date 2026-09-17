//! m0005 — `endpoints` (a tag-expression selection of tools, exposed at
//! `POST /mcp/{slug}`), `endpoint_aliases` (rename a selected api_call/script as seen
//! through this endpoint), `endpoint_auth_providers` — I5's human-set join: which
//! auth providers an endpoint may bind to — and `service_token_endpoints`, the join a
//! self-service token's grant list lives in (`store::service_token`,
//! `server::auth::authenticate_mcp`). No rows for a token means unrestricted (every
//! endpoint, the permissive GitHub-classic-PAT-style default); a non-empty row set means
//! exactly those endpoints. Cascades both ways: deleting a token drops its grants
//! (`fk_service_token_endpoints_token`), and deleting an endpoint drops any grant that
//! named it (`fk_service_token_endpoints_endpoint`) rather than leaving one dangling.
//! Amended into this migration (not a new one) because the branch that added self-service
//! tokens is unmerged and nothing built on it is deployed yet.
//!
//! `endpoints.owner_id` is a real column, same reasoning as `m0003_definitions`'s own doc —
//! amended in here for the same unmerged-branch reason.

use sea_orm_migration::prelude::*;

use super::helpers::{timestamptz_now, uuid_col, uuid_pk};

/// Redeclared from `m0001_init`'s own table — see `migration`'s module doc.
#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0005_endpoints"
    }
}

#[derive(DeriveIden)]
enum AuthProviders {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Endpoints {
    Table,
    Id,
    OwnerId,
    Slug,
    TagExpr,
    WriteCeiling,
    Budgets,
    Instructions,
    Enabled,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum EndpointAliases {
    Table,
    Id,
    EndpointId,
    TargetKind,
    TargetSlug,
    Alias,
}

#[derive(DeriveIden)]
enum EndpointAuthProviders {
    Table,
    EndpointId,
    AuthProviderId,
}

/// Redeclared from `m0001_init`'s own table, not imported — see `migration`'s module doc for
/// why every migration keeps its own copy of any `Iden` enum it needs.
#[derive(DeriveIden)]
enum ServiceTokens {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ServiceTokenEndpoints {
    Table,
    ServiceTokenId,
    EndpointId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Endpoints::Table)
                    .if_not_exists()
                    .col(uuid_pk(Endpoints::Id))
                    .col(uuid_col(Endpoints::OwnerId))
                    .col(ColumnDef::new(Endpoints::Slug).text().not_null())
                    .col(ColumnDef::new(Endpoints::TagExpr).text().not_null())
                    .col(
                        ColumnDef::new(Endpoints::WriteCeiling)
                            .text()
                            .not_null()
                            .default("read"),
                    )
                    .col(
                        ColumnDef::new(Endpoints::Budgets)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(Endpoints::Instructions).text().null())
                    .col(
                        ColumnDef::new(Endpoints::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(timestamptz_now(Endpoints::CreatedAt))
                    .col(timestamptz_now(Endpoints::UpdatedAt))
                    .check(Expr::col(Endpoints::WriteCeiling).is_in(["read", "write"]))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_endpoints_owner")
                            .from(Endpoints::Table, Endpoints::OwnerId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_endpoints_owner_slug")
                    .table(Endpoints::Table)
                    .col(Endpoints::OwnerId)
                    .col(Endpoints::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(EndpointAliases::Table)
                    .if_not_exists()
                    .col(uuid_pk(EndpointAliases::Id))
                    .col(uuid_col(EndpointAliases::EndpointId))
                    .col(
                        ColumnDef::new(EndpointAliases::TargetKind)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EndpointAliases::TargetSlug)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EndpointAliases::Alias).text().not_null())
                    .check(Expr::col(EndpointAliases::TargetKind).is_in(["api_call", "script"]))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_endpoint_aliases_endpoint")
                            .from(EndpointAliases::Table, EndpointAliases::EndpointId)
                            .to(Endpoints::Table, Endpoints::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_endpoint_aliases_endpoint_alias")
                    .table(EndpointAliases::Table)
                    .col(EndpointAliases::EndpointId)
                    .col(EndpointAliases::Alias)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_endpoint_aliases_endpoint_target")
                    .table(EndpointAliases::Table)
                    .col(EndpointAliases::EndpointId)
                    .col(EndpointAliases::TargetKind)
                    .col(EndpointAliases::TargetSlug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(EndpointAuthProviders::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EndpointAuthProviders::EndpointId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EndpointAuthProviders::AuthProviderId)
                            .uuid()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(EndpointAuthProviders::EndpointId)
                            .col(EndpointAuthProviders::AuthProviderId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_endpoint_auth_providers_endpoint")
                            .from(
                                EndpointAuthProviders::Table,
                                EndpointAuthProviders::EndpointId,
                            )
                            .to(Endpoints::Table, Endpoints::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_endpoint_auth_providers_provider")
                            .from(
                                EndpointAuthProviders::Table,
                                EndpointAuthProviders::AuthProviderId,
                            )
                            .to(AuthProviders::Table, AuthProviders::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ServiceTokenEndpoints::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ServiceTokenEndpoints::ServiceTokenId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ServiceTokenEndpoints::EndpointId)
                            .uuid()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(ServiceTokenEndpoints::ServiceTokenId)
                            .col(ServiceTokenEndpoints::EndpointId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_token_endpoints_token")
                            .from(
                                ServiceTokenEndpoints::Table,
                                ServiceTokenEndpoints::ServiceTokenId,
                            )
                            .to(ServiceTokens::Table, ServiceTokens::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_token_endpoints_endpoint")
                            .from(
                                ServiceTokenEndpoints::Table,
                                ServiceTokenEndpoints::EndpointId,
                            )
                            .to(Endpoints::Table, Endpoints::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ServiceTokenEndpoints::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(EndpointAuthProviders::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(EndpointAliases::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Endpoints::Table).to_owned())
            .await?;
        Ok(())
    }
}
