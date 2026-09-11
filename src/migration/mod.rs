//! SeaORM migrations, one module per migration, run by `api2mcp migrate` and
//! automatically at server startup under a Postgres advisory lock
//! ([`crate::db::run_migrations_locked`]).
//!
//! Each `mNNNN` file is self-contained: it redeclares any `Iden` enum it needs from an
//! earlier migration's table (see `m0002_oauth`'s note) rather than importing it, because
//! a shipped migration is append-only and must keep compiling unchanged forever, even
//! after a later migration or an entity refactor moves the "real" identifiers.

mod helpers;
mod m0001_init;
mod m0002_oauth;
mod m0003_definitions;
mod m0004_scripts_tags;
mod m0005_endpoints;
mod m0006_runs;
mod m0007_seed_first_user;

pub use sea_orm_migration::MigratorTrait;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn sea_orm_migration::MigrationTrait>> {
        vec![
            Box::new(m0001_init::Migration),
            Box::new(m0002_oauth::Migration),
            Box::new(m0003_definitions::Migration),
            Box::new(m0004_scripts_tags::Migration),
            Box::new(m0005_endpoints::Migration),
            Box::new(m0006_runs::Migration),
            Box::new(m0007_seed_first_user::Migration),
        ]
    }
}
