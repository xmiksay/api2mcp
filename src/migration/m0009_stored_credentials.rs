//! m0009 — `auth_providers.credential_value`: a per-owner credential held on the row instead of
//! in the server's environment.
//!
//! Why the schema now holds a credential at all: every definition here is owned, and each owner
//! needs their own token for the same upstream. A process-wide `A2M_CRED_*` env var cannot
//! express that, and adding a user would mean editing the server's environment and restarting it.
//! The value is stored in plaintext — a deliberate, accepted trade, documented in
//! `model::auth`'s module doc: anyone with read access to this database, or to a backup of it,
//! has every stored credential.
//!
//! `credential_env_key` becomes nullable in the same step, because a provider now names exactly
//! one source: the env var, or the column. Existing rows all have an env key and no stored value,
//! so they keep resolving exactly as before — this migration changes no row's behaviour.

use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
enum AuthProviders {
    Table,
    CredentialEnvKey,
    CredentialValue,
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0009_stored_credentials"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AuthProviders::Table)
                    .add_column(ColumnDef::new(AuthProviders::CredentialValue).text().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AuthProviders::Table)
                    .modify_column(
                        ColumnDef::new(AuthProviders::CredentialEnvKey)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Dropping the column discards every stored credential; an owner must re-enter one
        // against an env var instead. Reverting the nullability would fail on any row that
        // has no env key, so `down` leaves `credential_env_key` nullable.
        manager
            .alter_table(
                Table::alter()
                    .table(AuthProviders::Table)
                    .drop_column(AuthProviders::CredentialValue)
                    .to_owned(),
            )
            .await
    }
}
