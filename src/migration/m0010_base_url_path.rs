//! m0010 — a data migration for issue #6, when `http::bind` began prepending `services.base_url`'s
//! path to every api_call's `path_template` instead of silently discarding it. Rows written
//! before then are rewritten so each one keeps requesting what it requested before, with one
//! inexpressible exception noted below:
//!
//! - **The query string and fragment are stripped from `base_url`.** `bind` never sent either,
//!   and `pack::validate` now refuses both, so an existing row that kept one would fail
//!   whole-owner revalidation on every unrelated write and break re-import of its own export.
//! - **An api_call that already spelled out the base path loses it.** With base
//!   `https://host/api/v1`, the template `/api/v1/tenants` worked around the old behaviour and
//!   would now request `/api/v1/api/v1/tenants`; it becomes `/tenants`, an unchanged request.
//!   A template that does *not* start with the base path is exactly the case issue #6 reported
//!   as broken, and the new behaviour fixes it, so it is left alone.
//! - **The exception: a template equal to the base path** (`/api/v1`) becomes `/`, which now
//!   requests `/api/v1/` — the new model has no way to say "the base path, no trailing slash".
//!   It is the closest expressible rewrite, and each such api_call is logged at `warn` (owner id
//!   and slug) so an operator can check the upstream still answers it.
//!
//! Only a template that spells the prefix byte-for-byte as the stored base path is matched. One
//! that spells it differently — through a placeholder (`/api/{ver}/x`), a different
//! percent-encoding, or different case — is not rewritten and now gets the prefix twice.
//!
//! Raw SQL, not an entity `ActiveModel`, for the reason `m0007` gives. `down` is a no-op: the
//! stripped query/fragment is gone, and re-adding a prefix would only reproduce the bug.

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;
use uuid::Uuid;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0010_base_url_path"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let services = conn
            .query_all(Statement::from_string(
                DbBackend::Postgres,
                "SELECT id, base_url FROM services",
            ))
            .await?;
        for row in services {
            let id: Uuid = row.try_get("", "id")?;
            let raw: String = row.try_get("", "base_url")?;
            // A row that does not parse was never callable, and is no worse off left as it is.
            let Ok(mut base_url) = url::Url::parse(&raw) else {
                continue;
            };
            if base_url.query().is_some() || base_url.fragment().is_some() {
                base_url.set_query(None);
                base_url.set_fragment(None);
                conn.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE services SET base_url = $1 WHERE id = $2",
                    [base_url.to_string().into(), id.into()],
                ))
                .await?;
            }
            strip_prefix_from_api_calls(conn, id, base_path_prefix(&base_url)).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn strip_prefix_from_api_calls(
    conn: &impl ConnectionTrait,
    service_id: Uuid,
    prefix: &str,
) -> Result<(), DbErr> {
    if prefix.is_empty() {
        return Ok(());
    }
    let calls = conn
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, owner_id, slug, path_template FROM api_calls WHERE service_id = $1",
            [service_id.into()],
        ))
        .await?;
    for row in calls {
        let id: Uuid = row.try_get("", "id")?;
        let template: String = row.try_get("", "path_template")?;
        if let Some(stripped) = strip_base_prefix(prefix, &template) {
            if template == prefix {
                let owner_id: Uuid = row.try_get("", "owner_id")?;
                let slug: String = row.try_get("", "slug")?;
                tracing::warn!(
                    %owner_id,
                    api_call = %slug,
                    "m0010: path_template equal to its service's base path rewritten to \"/\"; \
                     it now requests the base path with a trailing slash — check the upstream \
                     still answers it"
                );
            }
            conn.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE api_calls SET path_template = $1 WHERE id = $2",
                [stripped.into(), id.into()],
            ))
            .await?;
        }
    }
    Ok(())
}

/// The same prefix `http::bind` prepends: the base path minus one trailing `/`, so `/` yields
/// the empty prefix. Duplicated rather than imported because a shipped migration must keep
/// compiling unchanged even if `bind` later moves on.
fn base_path_prefix(base_url: &url::Url) -> &str {
    let path = base_url.path();
    path.strip_suffix('/').unwrap_or(path)
}

/// `Some(rewritten)` when `template` already starts with `prefix` on a segment boundary.
fn strip_base_prefix(prefix: &str, template: &str) -> Option<String> {
    let rest = template.strip_prefix(prefix)?;
    if rest.is_empty() {
        Some("/".to_owned())
    } else if rest.starts_with('/') {
        Some(rest.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_spelling_out_the_base_path_loses_it() {
        assert_eq!(
            strip_base_prefix("/api/v1", "/api/v1/tenants/{id}").as_deref(),
            Some("/tenants/{id}")
        );
        // Not behaviour-preserving: `/` now requests `/api/v1/`, not `/api/v1` (see module doc).
        assert_eq!(
            strip_base_prefix("/api/v1", "/api/v1").as_deref(),
            Some("/")
        );
    }

    #[test]
    fn a_template_without_the_base_path_is_left_alone() {
        assert_eq!(strip_base_prefix("/api/v1", "/tenants"), None);
        // Not a segment boundary: `/api/v10` is not under `/api/v1`.
        assert_eq!(strip_base_prefix("/api/v1", "/api/v10/x"), None);
    }

    #[test]
    fn the_prefix_matches_what_bind_prepends() {
        for (base, prefix) in [
            ("https://h", ""),
            ("https://h/", ""),
            ("https://h/api/v1", "/api/v1"),
            ("https://h/api/v1/", "/api/v1"),
        ] {
            let url = url::Url::parse(base).expect("valid url");
            assert_eq!(base_path_prefix(&url), prefix, "{base}");
        }
    }
}
