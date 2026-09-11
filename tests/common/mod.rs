//! Scratch-Postgres harness shared by integration tests. `#[allow(dead_code)]` at the
//! crate root of this module: later test binaries (http_upstream, redaction, ...) will use
//! helpers this one does not, and each `tests/*.rs` file compiles as its own crate, so an
//! unused item here would otherwise warn per binary that doesn't need it yet.
#![allow(dead_code)]

use anyhow::{Context, Result};
use api2mcp::db;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use uuid::Uuid;

/// A scratch Postgres database created for one test. Every test using this harness must
/// check [`ScratchDb::create`]'s `None` case and return early — that's what keeps `cargo
/// test` green on a machine with no `TEST_DATABASE_URL`/Postgres.
pub struct ScratchDb {
    pub conn: DatabaseConnection,
    admin_url: String,
    name: String,
}

impl ScratchDb {
    /// Reads `TEST_DATABASE_URL`, creates `api2mcp_test_<uuid>` on the same server and
    /// connects to it. Returns `Ok(None)` when the env var is unset or blank.
    pub async fn create() -> Result<Option<Self>> {
        let Some(base_url) = std::env::var("TEST_DATABASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            return Ok(None);
        };

        let admin_url = with_db_name(&base_url, "postgres")?;
        let name = format!("api2mcp_test_{}", Uuid::new_v4().simple());

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

        Ok(Some(Self {
            conn,
            admin_url,
            name,
        }))
    }

    /// Runs every migration, exactly the path the server takes at startup.
    pub async fn migrate_up(&self) -> Result<()> {
        db::run_migrations_locked(&self.conn).await
    }

    /// Closes the scratch connection and drops the database. Must be called explicitly —
    /// a `Drop` impl can't run the required `async` `DROP DATABASE`.
    ///
    /// Cleanup failure is a warning, not a test failure. By the time this runs the test's
    /// assertions have already passed, so turning a green test red over a disposable,
    /// uniquely-named database would be pure noise. `WITH (FORCE)` terminates every backend
    /// still attached, which intermittently trips "permission denied to terminate process"
    /// when one of them is not owned by our role (an autovacuum worker, say) — hence the
    /// retries and the soft landing.
    pub async fn teardown(self) -> Result<()> {
        self.conn
            .close()
            .await
            .context("closing the scratch connection")?;

        let admin = db::connect(&self.admin_url)
            .await
            .context("reconnecting to the admin (postgres) database to drop the scratch db")?;

        let stmt = format!(r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#, self.name);
        let mut last_err = None;
        for attempt in 0..3 {
            match admin
                .execute(Statement::from_string(admin.get_database_backend(), &stmt))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(50 * (attempt + 1))).await;
                }
            }
        }
        eprintln!(
            "warning: could not drop scratch database {} ({}); it is disposable and safe to \
             remove by hand",
            self.name,
            last_err.map(|e| e.to_string()).unwrap_or_default()
        );
        Ok(())
    }
}

/// Replace the trailing `/<database>` path of a Postgres URL, keeping the rest
/// (credentials, host, port, query string) intact.
fn with_db_name(url: &str, database: &str) -> Result<String> {
    let mut parsed = url::Url::parse(url).with_context(|| format!("parsing {url:?} as a URL"))?;
    parsed.set_path(&format!("/{database}"));
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::with_db_name;

    #[test]
    fn db_name_replacement_keeps_credentials_and_host() {
        let got =
            with_db_name("postgres://u:p@host:5432/orig?sslmode=disable", "postgres").unwrap();
        assert_eq!(got, "postgres://u:p@host:5432/postgres?sslmode=disable");
    }
}
