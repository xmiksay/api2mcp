//! m0003 — the first half of the tool-definition schema: `services` (one upstream +
//! its allowlist and limits), `auth_providers` (credential *name*, never a value — I4's
//! structural half), `api_calls` (one HTTP call) and `api_call_params` (its typed
//! inputs). `scripts`/`script_api_calls`/`tags` follow in [`super::m0004_scripts_tags`]
//! — split out to keep both files under the line cap.
//!
//! `services.origin_allowlist` and `auth_providers.scopes` are `JSONB` rather than the
//! plan's literal `TEXT[]`, for the same `postgres-array`-feature reason as m0001's
//! `service_tokens.scopes`.
//!
//! `auth_providers.slug` and `api_calls.slug` are `UNIQUE` globally, not per-service: a
//! bare slug is what `script_api_calls`, `endpoint_aliases.target_slug` and
//! `endpoint_auth_providers` all reference, and pack import/export round-trips the same
//! bare form. A per-service index would make that reference ambiguous whenever two
//! services defined the same slug — a runtime failure standing in for a constraint the
//! database can enforce outright. Slugs are already service-qualified by convention
//! (`gitlab.mr.list`), so global uniqueness costs nothing in practice.

use sea_orm_migration::prelude::*;

use super::helpers::{timestamptz_now, uuid_col, uuid_col_null, uuid_pk};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0003_definitions"
    }
}

#[derive(DeriveIden)]
enum Services {
    Table,
    Id,
    Slug,
    BaseUrl,
    OriginAllowlist,
    DefaultHeaders,
    TimeoutMs,
    MaxConcurrency,
    RateLimitPerMin,
    MaxResponseBytes,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum AuthProviders {
    Table,
    Id,
    ServiceId,
    Slug,
    Kind,
    CredentialEnvKey,
    HeaderName,
    ValueTemplate,
    Scopes,
    TokenUrl,
    BoundOrigin,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ApiCalls {
    Table,
    Id,
    ServiceId,
    AuthProviderId,
    Slug,
    Method,
    PathTemplate,
    QueryFixed,
    BodyTemplate,
    Access,
    Idempotent,
    Projection,
    Pagination,
    TimeoutMs,
    MaxResponseBytes,
    Description,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ApiCallParams {
    Table,
    Id,
    ApiCallId,
    Name,
    Location,
    DataType,
    Required,
    DefaultValue,
    FixedValue,
    EnumValues,
    BodyPath,
    Position,
    Description,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Services::Table)
                    .if_not_exists()
                    .col(uuid_pk(Services::Id))
                    .col(ColumnDef::new(Services::Slug).text().not_null())
                    .col(ColumnDef::new(Services::BaseUrl).text().not_null())
                    .col(
                        ColumnDef::new(Services::OriginAllowlist)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Services::DefaultHeaders)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(Services::TimeoutMs).integer().not_null())
                    .col(
                        ColumnDef::new(Services::MaxConcurrency)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Services::RateLimitPerMin)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Services::MaxResponseBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(timestamptz_now(Services::CreatedAt))
                    .col(timestamptz_now(Services::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_services_slug")
                    .table(Services::Table)
                    .col(Services::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AuthProviders::Table)
                    .if_not_exists()
                    .col(uuid_pk(AuthProviders::Id))
                    .col(uuid_col(AuthProviders::ServiceId))
                    .col(ColumnDef::new(AuthProviders::Slug).text().not_null())
                    .col(ColumnDef::new(AuthProviders::Kind).text().not_null())
                    .col(
                        ColumnDef::new(AuthProviders::CredentialEnvKey)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AuthProviders::HeaderName).text().null())
                    .col(ColumnDef::new(AuthProviders::ValueTemplate).text().null())
                    .col(ColumnDef::new(AuthProviders::Scopes).json_binary().null())
                    .col(ColumnDef::new(AuthProviders::TokenUrl).text().null())
                    .col(ColumnDef::new(AuthProviders::BoundOrigin).text().not_null())
                    .col(timestamptz_now(AuthProviders::CreatedAt))
                    .col(timestamptz_now(AuthProviders::UpdatedAt))
                    .check(Expr::col(AuthProviders::Kind).is_in([
                        "header",
                        "bearer",
                        "oauth2_client_credentials",
                    ]))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_auth_providers_service")
                            .from(AuthProviders::Table, AuthProviders::ServiceId)
                            .to(Services::Table, Services::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_auth_providers_slug")
                    .table(AuthProviders::Table)
                    .col(AuthProviders::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiCalls::Table)
                    .if_not_exists()
                    .col(uuid_pk(ApiCalls::Id))
                    .col(uuid_col(ApiCalls::ServiceId))
                    .col(uuid_col_null(ApiCalls::AuthProviderId))
                    .col(ColumnDef::new(ApiCalls::Slug).text().not_null())
                    .col(ColumnDef::new(ApiCalls::Method).text().not_null())
                    .col(ColumnDef::new(ApiCalls::PathTemplate).text().not_null())
                    .col(
                        ColumnDef::new(ApiCalls::QueryFixed)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(ApiCalls::BodyTemplate).json_binary().null())
                    .col(ColumnDef::new(ApiCalls::Access).text().not_null())
                    .col(
                        ColumnDef::new(ApiCalls::Idempotent)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ApiCalls::Projection).json_binary().null())
                    .col(ColumnDef::new(ApiCalls::Pagination).json_binary().null())
                    .col(ColumnDef::new(ApiCalls::TimeoutMs).integer().null())
                    .col(
                        ColumnDef::new(ApiCalls::MaxResponseBytes)
                            .big_integer()
                            .null(),
                    )
                    // What `server::mcp::registry::describe` uses verbatim as the MCP tool's
                    // `description` when present — the single highest-leverage field in the
                    // whole schema, since it is what a model reads to decide whether and how to
                    // call the tool. `NOT NULL DEFAULT ''` for the same reason as
                    // `api_call_params.description`: "no description" and "empty description"
                    // are the same thing to a schema consumer, so the store layer collapses `''`
                    // to `None` rather than carrying a third state through `model::ApiCall`.
                    .col(
                        ColumnDef::new(ApiCalls::Description)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(timestamptz_now(ApiCalls::CreatedAt))
                    .col(timestamptz_now(ApiCalls::UpdatedAt))
                    .check(
                        Expr::col(ApiCalls::Method)
                            .is_in(["GET", "POST", "PUT", "PATCH", "DELETE"]),
                    )
                    .check(Expr::col(ApiCalls::Access).is_in(["read", "write"]))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_calls_service")
                            .from(ApiCalls::Table, ApiCalls::ServiceId)
                            .to(Services::Table, Services::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_calls_auth_provider")
                            .from(ApiCalls::Table, ApiCalls::AuthProviderId)
                            .to(AuthProviders::Table, AuthProviders::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_api_calls_slug")
                    .table(ApiCalls::Table)
                    .col(ApiCalls::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiCallParams::Table)
                    .if_not_exists()
                    .col(uuid_pk(ApiCallParams::Id))
                    .col(uuid_col(ApiCallParams::ApiCallId))
                    .col(ColumnDef::new(ApiCallParams::Name).text().not_null())
                    .col(ColumnDef::new(ApiCallParams::Location).text().not_null())
                    .col(ColumnDef::new(ApiCallParams::DataType).text().not_null())
                    .col(
                        ColumnDef::new(ApiCallParams::Required)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ApiCallParams::DefaultValue)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ApiCallParams::FixedValue)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ApiCallParams::EnumValues)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(ApiCallParams::BodyPath).text().null())
                    // Stable schema-generation order (I7): input_schema and fan-out both
                    // iterate params by this column, never by insertion or name order.
                    .col(ColumnDef::new(ApiCallParams::Position).integer().not_null())
                    // What `schema::input_schema` emits as the property's `description` —
                    // the only per-param documentation a model sees. `NOT NULL DEFAULT ''`
                    // rather than nullable: "no description" and "empty description" are
                    // the same thing to a schema consumer, so there's no reason to carry a
                    // third (NULL) state through the store layer.
                    .col(
                        ColumnDef::new(ApiCallParams::Description)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .check(
                        Expr::col(ApiCallParams::Location)
                            .is_in(["path", "query", "header", "body"]),
                    )
                    .check(
                        Expr::col(ApiCallParams::DataType)
                            .is_in(["string", "integer", "number", "boolean", "array", "object"]),
                    )
                    .check(
                        Expr::col(ApiCallParams::FixedValue)
                            .is_null()
                            .or(Expr::col(ApiCallParams::Required).eq(false)),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_call_params_api_call")
                            .from(ApiCallParams::Table, ApiCallParams::ApiCallId)
                            .to(ApiCalls::Table, ApiCalls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_api_call_params_call_name")
                    .table(ApiCallParams::Table)
                    .col(ApiCallParams::ApiCallId)
                    .col(ApiCallParams::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_api_call_params_call_position")
                    .table(ApiCallParams::Table)
                    .col(ApiCallParams::ApiCallId)
                    .col(ApiCallParams::Position)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ApiCallParams::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ApiCalls::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AuthProviders::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Services::Table).to_owned())
            .await?;
        Ok(())
    }
}
