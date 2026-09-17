//! m0007 — seed the very first user from `A2M_SEED_EMAIL`/`A2M_SEED_PASSWORD`, so a fresh
//! deployment has a way into the server-rendered login without a manual `INSERT`. There is no
//! "admin" left to seed (see `entity::users`'s own doc): every user can read and write every
//! definition, so this just creates *a* user, not a privileged one.
//!
//! Idempotent: a no-op when either variable is unset, or when a `users` row already exists (an
//! operator who already created an account should never be silently handed a second one, or
//! have their own password overwritten by a stale env var).
//!
//! Raw SQL rather than an [`crate::entity::users`] `ActiveModel`: a migration must keep working
//! unchanged even after the entity's shape moves on, since migrations are append-only and never
//! edited again. Renamed from `m0007_seed_admin` in place (not appended as a corrective
//! migration) — the branch is unmerged and nothing is deployed yet.

use anyhow::Context;
use argon2::{Argon2, PasswordHasher};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0007_seed_first_user"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let (Some(email), Some(password)) = (
            non_empty_env("A2M_SEED_EMAIL"),
            non_empty_env("A2M_SEED_PASSWORD"),
        ) else {
            return Ok(());
        };

        let conn = manager.get_connection();
        if user_count(conn).await? > 0 {
            return Ok(());
        }

        let hash = hash_password(&password)
            .map_err(|e| DbErr::Custom(format!("hashing seed user password: {e:#}")))?;

        conn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (email, password_hash) VALUES ($1, $2)",
            [email.into(), hash.into()],
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // See the module doc: if `up` was a no-op because a user already existed, this
        // still deletes whatever row now has this email. Acceptable for a best-effort
        // dev/first-boot seed — the migration round-trip test never sets these env vars,
        // so both directions are no-ops there.
        let Some(email) = non_empty_env("A2M_SEED_EMAIL") else {
            return Ok(());
        };
        manager
            .get_connection()
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM users WHERE email = $1",
                [email.into()],
            ))
            .await?;
        Ok(())
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

async fn user_count(conn: &impl ConnectionTrait) -> Result<i64, DbErr> {
    let row = conn
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS n FROM users",
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("SELECT COUNT(*) returned no row".into()))?;
    row.try_get::<i64>("", "n")
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("argon2id hashing")?
        .to_string())
}
