//! `api2mcp export --endpoint <slug>` writes YAML to stdout; `api2mcp import <path>
//! [--dry-run]` validates and imports it. Validation always runs first and reports every
//! failure it finds — see [`crate::pack::validate`] — so a hand-written pack doesn't need
//! repeated fix-and-retry cycles to surface every problem.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::db;
use crate::model::Slug;
use crate::pack::ImportChange;
use crate::store::Stores;

pub async fn export(endpoint: &str, user: Option<&str>) -> Result<()> {
    let slug: Slug = endpoint
        .parse()
        .with_context(|| format!("{endpoint:?} is not a valid slug"))?;
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;
    let stores = Stores::new(conn);
    let owner = crate::cli::resolve_user(&stores, user).await?;

    let pack = crate::pack::export_endpoint(&stores, owner.id, &slug)
        .await
        .with_context(|| format!("exporting endpoint {endpoint:?}"))?;
    let yaml = serde_norway::to_string(&pack).context("serializing pack as YAML")?;
    print!("{yaml}");
    Ok(())
}

pub async fn import(path: &Path, dry_run: bool, user: Option<&str>) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: crate::pack::Pack = serde_norway::from_str(&text)
        .with_context(|| format!("parsing {} as a pack", path.display()))?;

    if let Err(errors) = crate::pack::validate(&parsed) {
        eprintln!(
            "{} failed validation ({} problem(s)):",
            path.display(),
            errors.len()
        );
        for e in &errors {
            eprintln!("  - {e}");
        }
        bail!("validation failed, nothing imported");
    }

    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;
    let stores = Stores::new(conn);
    let owner = crate::cli::resolve_user(&stores, user).await?;

    let report = crate::pack::import(&stores, &parsed, dry_run, owner.id)
        .await
        .with_context(|| format!("importing {}", path.display()))?;

    if dry_run {
        println!("dry run — nothing written");
    }
    print_rows("services", &report.services);
    print_rows("auth_providers", &report.auth_providers);
    print_rows("api_calls", &report.api_calls);
    print_rows("scripts", &report.scripts);
    print_rows("endpoints", &report.endpoints);
    Ok(())
}

fn print_rows(label: &str, rows: &[(String, ImportChange)]) {
    if rows.is_empty() {
        return;
    }
    println!("{label}:");
    for (slug, change) in rows {
        let verb = match change {
            ImportChange::Created => "create",
            ImportChange::Updated => "update",
            ImportChange::Unchanged => "unchanged",
        };
        println!("  {verb:<9} {slug}");
    }
}
