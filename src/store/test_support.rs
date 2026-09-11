//! A minimal scratch-Postgres harness for `#[cfg(test)]` unit tests *inside* this crate —
//! needed because `store::auth_provider`'s write methods are `pub(crate)` (I5): an
//! integration test under `tests/` is a separate crate and can't see them at all, so the
//! only place they can be exercised is a unit test here. Deliberately a smaller duplicate of
//! `tests/common::ScratchDb` rather than a shared dependency between the two — `tests/` is a
//! separate compilation unit from the library and can't be imported from it.

use anyhow::{Context, Result};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::db;

pub(crate) struct ScratchDb {
    pub(crate) db: DatabaseConnection,
    admin_url: String,
    name: String,
}

impl ScratchDb {
    /// `Ok(None)` when `TEST_DATABASE_URL` is unset/blank — every caller must check this and
    /// return early, exactly like `tests/common::ScratchDb::create`.
    pub(crate) async fn create() -> Result<Option<Self>> {
        let Some(base_url) = std::env::var("TEST_DATABASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            return Ok(None);
        };

        let admin_url = with_db_name(&base_url, "postgres")?;
        let name = format!("api2mcp_store_test_{}", Uuid::new_v4().simple());

        let admin = db::connect(&admin_url)
            .await
            .context("connecting to the admin (postgres) database")?;
        admin
            .execute(Statement::from_string(
                admin.get_database_backend(),
                format!(r#"CREATE DATABASE "{name}""#),
            ))
            .await
            .with_context(|| format!("creating scratch database {name}"))?;
        admin
            .close()
            .await
            .context("closing the admin connection")?;

        let scratch_url = with_db_name(&base_url, &name)?;
        let conn = db::connect(&scratch_url)
            .await
            .with_context(|| format!("connecting to scratch database {name}"))?;
        db::run_migrations_locked(&conn).await?;

        Ok(Some(Self {
            db: conn,
            admin_url,
            name,
        }))
    }

    pub(crate) async fn teardown(self) -> Result<()> {
        self.db
            .close()
            .await
            .context("closing the scratch connection")?;
        let admin = db::connect(&self.admin_url)
            .await
            .context("reconnecting to the admin (postgres) database to drop the scratch db")?;
        admin
            .execute(Statement::from_string(
                admin.get_database_backend(),
                format!(r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#, self.name),
            ))
            .await
            .with_context(|| format!("dropping scratch database {}", self.name))?;
        Ok(())
    }
}

fn with_db_name(url: &str, database: &str) -> Result<String> {
    let mut parsed = url::Url::parse(url).with_context(|| format!("parsing {url:?} as a URL"))?;
    parsed.set_path(&format!("/{database}"));
    Ok(parsed.into())
}
