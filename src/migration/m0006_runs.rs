//! m0006 — the audit trail. `runs` carries a full `definition_snapshot` of the tool as
//! it executed (I8 was dropped, so this is the only record of "what did this tool look
//! like when it ran") plus its sha256 `definition_digest`; `run_calls` is one row per
//! upstream request a run made, keyed by `seq` — the **input index of the fan-out, not
//! its completion index** (I7 made durable: completion order is never observable, so it
//! must never be recorded either).
//!
//! `runs.owner_id` — a user sees only runs against their own definitions. `endpoint_slug`
//! alone can no longer answer "whose run is this": slugs are unique per owner
//! (`m0003_definitions`'s own doc), so two different people's endpoints can share a slug.
//! Set from the resolved `EndpointPlan::owner_id` (the endpoint definition's own owner) at
//! record time, not from `caller_id` — that column is a free-form string (`"cli"`,
//! `"admin-test:<uuid>"`, a token owner's uuid, ...) with no single parseable shape, where
//! `owner_id` is a plain, always-present foreign key. Amended into this migration (not a new
//! one) — same unmerged-branch reasoning as `m0003`.

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
        "m0006_runs"
    }
}

#[derive(DeriveIden)]
enum Runs {
    Table,
    Id,
    OwnerId,
    EndpointSlug,
    ToolName,
    TargetKind,
    TargetSlug,
    CallerKind,
    CallerId,
    RequestId,
    ExecutionStart,
    DefinitionSnapshot,
    DefinitionDigest,
    InputRedacted,
    OutputRedacted,
    Status,
    Errors,
    CallsMade,
    BytesIn,
    PagesFetched,
    BudgetSnapshot,
    Timings,
    CreatedAt,
}

#[derive(DeriveIden)]
enum RunCalls {
    Table,
    Id,
    RunId,
    Seq,
    ApiCallSlug,
    ServiceSlug,
    Method,
    UrlRedacted,
    HeadersRedacted,
    BodyRedacted,
    StatusCode,
    ResponseBytes,
    ResponseTruncated,
    Error,
    Timings,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Runs::Table)
                    .if_not_exists()
                    .col(uuid_pk(Runs::Id))
                    .col(uuid_col(Runs::OwnerId))
                    .col(ColumnDef::new(Runs::EndpointSlug).text().not_null())
                    .col(ColumnDef::new(Runs::ToolName).text().not_null())
                    .col(ColumnDef::new(Runs::TargetKind).text().not_null())
                    .col(ColumnDef::new(Runs::TargetSlug).text().not_null())
                    .col(ColumnDef::new(Runs::CallerKind).text().not_null())
                    .col(ColumnDef::new(Runs::CallerId).text().not_null())
                    .col(ColumnDef::new(Runs::RequestId).text().not_null())
                    // The single wall-clock instant `BudgetMeter::new` froze for this run, and
                    // the source of truth `script::dates::execution_start()` reads from — without
                    // it, a run record can't actually be replayed deterministically (I7's own
                    // wording), only claimed to be.
                    .col(
                        ColumnDef::new(Runs::ExecutionStart)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Runs::DefinitionSnapshot)
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Runs::DefinitionDigest).text().not_null())
                    .col(ColumnDef::new(Runs::InputRedacted).json_binary().not_null())
                    .col(ColumnDef::new(Runs::OutputRedacted).json_binary().null())
                    .col(ColumnDef::new(Runs::Status).text().not_null())
                    .col(ColumnDef::new(Runs::Errors).json_binary().null())
                    .col(
                        ColumnDef::new(Runs::CallsMade)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Runs::BytesIn)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Runs::PagesFetched)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Runs::BudgetSnapshot).json_binary().null())
                    .col(ColumnDef::new(Runs::Timings).json_binary().null())
                    .col(timestamptz_now(Runs::CreatedAt))
                    .check(Expr::col(Runs::TargetKind).is_in(["api_call", "script"]))
                    .check(Expr::col(Runs::CallerKind).is_in([
                        "session",
                        "oauth",
                        "service_token",
                        "cli",
                    ]))
                    .check(Expr::col(Runs::Status).is_in([
                        "ok",
                        "partial",
                        "error",
                        "denied",
                        "budget_exceeded",
                        "timeout",
                    ]))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_runs_owner")
                            .from(Runs::Table, Runs::OwnerId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ix_runs_owner_created_at")
                    .table(Runs::Table)
                    .col(Runs::OwnerId)
                    .col(Runs::CreatedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ix_runs_owner_endpoint_slug_created_at")
                    .table(Runs::Table)
                    .col(Runs::OwnerId)
                    .col(Runs::EndpointSlug)
                    .col(Runs::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(RunCalls::Table)
                    .if_not_exists()
                    .col(uuid_pk(RunCalls::Id))
                    .col(uuid_col(RunCalls::RunId))
                    .col(ColumnDef::new(RunCalls::Seq).integer().not_null())
                    .col(ColumnDef::new(RunCalls::ApiCallSlug).text().not_null())
                    .col(ColumnDef::new(RunCalls::ServiceSlug).text().not_null())
                    .col(ColumnDef::new(RunCalls::Method).text().not_null())
                    .col(ColumnDef::new(RunCalls::UrlRedacted).text().not_null())
                    .col(
                        ColumnDef::new(RunCalls::HeadersRedacted)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(RunCalls::BodyRedacted).json_binary().null())
                    .col(ColumnDef::new(RunCalls::StatusCode).integer().null())
                    .col(ColumnDef::new(RunCalls::ResponseBytes).big_integer().null())
                    .col(
                        ColumnDef::new(RunCalls::ResponseTruncated)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(RunCalls::Error).text().null())
                    .col(ColumnDef::new(RunCalls::Timings).json_binary().null())
                    .col(timestamptz_now(RunCalls::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_run_calls_run")
                            .from(RunCalls::Table, RunCalls::RunId)
                            .to(Runs::Table, Runs::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ux_run_calls_run_seq")
                    .table(RunCalls::Table)
                    .col(RunCalls::RunId)
                    .col(RunCalls::Seq)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RunCalls::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Runs::Table).to_owned())
            .await?;
        Ok(())
    }
}
