//! m0008 — `service_tokens.control_plane`: the explicit per-token capability that gates bare
//! `POST /mcp` (the definition-authoring factory, `server::mcp::control`), separate from
//! `restricted`/`service_token_endpoints` (which gate `/mcp/{slug}`, the data plane, and mean
//! nothing here — a token can be unrestricted on every endpoint and still lack this).
//!
//! `NOT NULL DEFAULT false`: every token minted before this migration stays data-plane only,
//! which is the whole point — reaching the factory is something a token must be minted *with*,
//! never something an existing token gains for free by an unrelated schema change.

use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
enum ServiceTokens {
    Table,
    ControlPlane,
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0008_control_plane_tokens"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ServiceTokens::Table)
                    .add_column(
                        ColumnDef::new(ServiceTokens::ControlPlane)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ServiceTokens::Table)
                    .drop_column(ServiceTokens::ControlPlane)
                    .to_owned(),
            )
            .await
    }
}
