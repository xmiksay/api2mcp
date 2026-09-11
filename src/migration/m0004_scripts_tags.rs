//! m0004 — Rhai `scripts` + their `script_params`, `script_api_calls` (I1's *declarative*
//! half: the fixed set of api_calls a script may reach, under a per-script alias —
//! `runtime::dispatch` resolves every `api()`/`api_many()` name through this join, and
//! nothing else can reach `http::send`), and `tags` + the two tag-membership joins that
//! `endpoints` select over.
//!
//! `script_params` mirrors `api_call_params`'s typed-input shape (`data_type`, `required`,
//! `default_value`, `enum_values`, `position`, `description`) but drops `location`/
//! `fixed_value`/`body_path`: those describe *where in an HTTP request* a value lands,
//! which is meaningless for a script input — a script receives its params as plain Rhai
//! bindings, not something bound into a template.
//!
//! `scripts` has no `projection` column: a script returns its own already-composed Rhai
//! value, so there is nothing here for a declarative JSONPath projection to act on — that
//! is strictly an `api_call` concept. Its budget is five nullable columns
//! (`max_calls`/`max_bytes`/`wall_clock_ms`/`max_pages`/`max_concurrency`), not one
//! `timeout_ms`: `Budgets::fold` (I6) is element-wise `min` across all five axes, so a
//! script that can only narrow `wall_clock` could never narrow the other four — the
//! invariant needs every axis representable here, not just one.

use sea_orm_migration::prelude::*;

use super::helpers::{timestamptz_now, uuid_col, uuid_pk};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0004_scripts_tags"
    }
}

#[derive(DeriveIden)]
enum ApiCalls {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Scripts {
    Table,
    Id,
    Slug,
    Description,
    Source,
    MaxCalls,
    MaxBytes,
    WallClockMs,
    MaxPages,
    MaxConcurrency,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ScriptParams {
    Table,
    Id,
    ScriptId,
    Name,
    DataType,
    Required,
    DefaultValue,
    EnumValues,
    Position,
    Description,
}

#[derive(DeriveIden)]
enum ScriptApiCalls {
    Table,
    Id,
    ScriptId,
    ApiCallId,
    Alias,
}

#[derive(DeriveIden)]
enum Tags {
    Table,
    Id,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ApiCallTags {
    Table,
    ApiCallId,
    TagId,
}

#[derive(DeriveIden)]
enum ScriptTags {
    Table,
    ScriptId,
    TagId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Scripts::Table)
                    .if_not_exists()
                    .col(uuid_pk(Scripts::Id))
                    .col(ColumnDef::new(Scripts::Slug).text().not_null())
                    .col(ColumnDef::new(Scripts::Description).text().null())
                    .col(ColumnDef::new(Scripts::Source).text().not_null())
                    // A script's own budget opinion (I6) — `None`/NULL means "no opinion
                    // on this axis", never "unlimited"; see `model::budget` for why that
                    // asymmetry is what keeps `Budgets::fold` narrowing-only.
                    .col(ColumnDef::new(Scripts::MaxCalls).integer().null())
                    .col(ColumnDef::new(Scripts::MaxBytes).big_integer().null())
                    .col(ColumnDef::new(Scripts::WallClockMs).integer().null())
                    .col(ColumnDef::new(Scripts::MaxPages).integer().null())
                    .col(ColumnDef::new(Scripts::MaxConcurrency).integer().null())
                    .col(timestamptz_now(Scripts::CreatedAt))
                    .col(timestamptz_now(Scripts::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_scripts_slug")
                    .table(Scripts::Table)
                    .col(Scripts::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ScriptParams::Table)
                    .if_not_exists()
                    .col(uuid_pk(ScriptParams::Id))
                    .col(uuid_col(ScriptParams::ScriptId))
                    .col(ColumnDef::new(ScriptParams::Name).text().not_null())
                    .col(ColumnDef::new(ScriptParams::DataType).text().not_null())
                    .col(
                        ColumnDef::new(ScriptParams::Required)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ScriptParams::DefaultValue)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ScriptParams::EnumValues)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(ScriptParams::Position).integer().not_null())
                    // Same role and default as `api_call_params.description` (m0003) — see
                    // that column's comment for why `NOT NULL DEFAULT ''` rather than
                    // nullable.
                    .col(
                        ColumnDef::new(ScriptParams::Description)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .check(
                        Expr::col(ScriptParams::DataType)
                            .is_in(["string", "integer", "number", "boolean", "array", "object"]),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_script_params_script")
                            .from(ScriptParams::Table, ScriptParams::ScriptId)
                            .to(Scripts::Table, Scripts::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_script_params_script_name")
                    .table(ScriptParams::Table)
                    .col(ScriptParams::ScriptId)
                    .col(ScriptParams::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_script_params_script_position")
                    .table(ScriptParams::Table)
                    .col(ScriptParams::ScriptId)
                    .col(ScriptParams::Position)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ScriptApiCalls::Table)
                    .if_not_exists()
                    .col(uuid_pk(ScriptApiCalls::Id))
                    .col(uuid_col(ScriptApiCalls::ScriptId))
                    .col(uuid_col(ScriptApiCalls::ApiCallId))
                    .col(ColumnDef::new(ScriptApiCalls::Alias).text().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_script_api_calls_script")
                            .from(ScriptApiCalls::Table, ScriptApiCalls::ScriptId)
                            .to(Scripts::Table, Scripts::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_script_api_calls_api_call")
                            .from(ScriptApiCalls::Table, ScriptApiCalls::ApiCallId)
                            .to(ApiCalls::Table, ApiCalls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_script_api_calls_script_alias")
                    .table(ScriptApiCalls::Table)
                    .col(ScriptApiCalls::ScriptId)
                    .col(ScriptApiCalls::Alias)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Tags::Table)
                    .if_not_exists()
                    .col(uuid_pk(Tags::Id))
                    .col(ColumnDef::new(Tags::Name).text().not_null())
                    .col(timestamptz_now(Tags::CreatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_tags_name")
                    .table(Tags::Table)
                    .col(Tags::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiCallTags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ApiCallTags::ApiCallId).uuid().not_null())
                    .col(ColumnDef::new(ApiCallTags::TagId).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(ApiCallTags::ApiCallId)
                            .col(ApiCallTags::TagId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_call_tags_api_call")
                            .from(ApiCallTags::Table, ApiCallTags::ApiCallId)
                            .to(ApiCalls::Table, ApiCalls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_call_tags_tag")
                            .from(ApiCallTags::Table, ApiCallTags::TagId)
                            .to(Tags::Table, Tags::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ScriptTags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ScriptTags::ScriptId).uuid().not_null())
                    .col(ColumnDef::new(ScriptTags::TagId).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(ScriptTags::ScriptId)
                            .col(ScriptTags::TagId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_script_tags_script")
                            .from(ScriptTags::Table, ScriptTags::ScriptId)
                            .to(Scripts::Table, Scripts::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_script_tags_tag")
                            .from(ScriptTags::Table, ScriptTags::TagId)
                            .to(Tags::Table, Tags::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ScriptTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ApiCallTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Tags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ScriptApiCalls::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ScriptParams::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Scripts::Table).to_owned())
            .await?;
        Ok(())
    }
}
