//! m0011 — `endpoints.internal_tag_expr`: a second, optional tag expression selecting api_calls
//! an endpoint's scripts may reach without those api_calls becoming tools of their own.
//!
//! Nullable with no default: every existing row reads back as "no internal selection", which is
//! exactly the pre-m0011 behaviour — this migration changes no row's behaviour.

use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
enum Endpoints {
    Table,
    InternalTagExpr,
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0011_internal_tag_expr"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Endpoints::Table)
                    .add_column(ColumnDef::new(Endpoints::InternalTagExpr).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Endpoints::Table)
                    .drop_column(Endpoints::InternalTagExpr)
                    .to_owned(),
            )
            .await
    }
}
