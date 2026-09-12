//! Imports a validated [`Pack`] into the database: upsert by slug, last write wins (this crate
//! doesn't version definitions — see `lib.rs`'s invariant table). Writes happen in FK dependency
//! order — services, auth_providers, api_calls, scripts, endpoints — because each step's upsert
//! resolves the previous steps' rows by slug (a script's `callable` needs its api_calls already
//! committed, an endpoint's `auth_providers` scope needs its providers already committed, ...).
//! Child rows (params, tag membership, `script_api_calls`, `endpoint_aliases`,
//! `endpoint_auth_providers`) are never patched incrementally: every aggregate's own `update()`
//! always replaces its full child-row set from the value just written
//! (`store::api_call::update`, `store::script::update`, `store::endpoint::update` all do this
//! already), so an import can never leave a stale param or tag membership behind.
//!
//! One thing this module deliberately does **not** achieve, documented here rather than glossed
//! over: a single Postgres transaction spanning the whole import, with
//! `meta.definitions_generation` bumped exactly once. Every `*Store::create`/`update` opens and
//! commits its own transaction on its own connection (e.g. `store::service::create` calls
//! `self.db.begin()`), with no way to hand it an already-open one, and each one bumps the
//! generation counter itself (`MetaStore::bump_generation_in`, inside that same per-row
//! transaction). Neither limitation is fixable from `pack/` alone — it would need a
//! transaction-accepting variant of every `*Store::create`/`update`, which is a `store/` change
//! outside this chunk's file list. Concretely, what that means for a caller:
//! - `meta.definitions_generation` bumps once per row written, not once per import. Harmless —
//!   every bump still invalidates `PlanCache` correctly, it's just more invalidation than the
//!   minimum — but it is not "exactly once", and this doc says so rather than a comment claiming
//!   otherwise.
//! - a failure partway through a multi-row import leaves the rows written so far committed; there
//!   is no outer rollback. [`super::validate::validate`] runs first, pure and DB-free, and catches
//!   every structural problem a pack can name before any of these per-row transactions opens —
//!   which is what keeps a partial import a rare failure mode, not a routine one.

use uuid::Uuid;

use crate::store::{StoreError, Stores};

use super::Pack;
use super::convert;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0}")]
    Convert(#[from] super::ConvertError),
    #[error("{0}")]
    Store(String),
}

impl From<StoreError> for ImportError {
    fn from(e: StoreError) -> Self {
        ImportError::Store(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportChange {
    Created,
    Updated,
    Unchanged,
}

/// What importing a pack did (or, under `dry_run`, would do) to each definition it names.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub services: Vec<(String, ImportChange)>,
    pub auth_providers: Vec<(String, ImportChange)>,
    pub api_calls: Vec<(String, ImportChange)>,
    pub scripts: Vec<(String, ImportChange)>,
    pub endpoints: Vec<(String, ImportChange)>,
}

impl ImportReport {
    /// `true` iff every row classified as [`ImportChange::Unchanged`] — a re-import of an
    /// already-imported pack should report exactly this.
    pub fn is_idempotent_no_op(&self) -> bool {
        [
            &self.services,
            &self.auth_providers,
            &self.api_calls,
            &self.scripts,
            &self.endpoints,
        ]
        .into_iter()
        .all(|rows| rows.iter().all(|(_, c)| *c == ImportChange::Unchanged))
    }
}

/// Upserts every definition in `pack`, owned by `owner_id` — "import assigns the importing user
/// as owner of everything it creates" (Decision). An update to a row that already existed keeps
/// its existing owner rather than being reassigned: every store lookup below is scoped to
/// `owner_id`, so a pack imported by a different user than the one who owns a same-slugged row
/// simply creates that user's own separate copy instead of colliding with it. `dry_run = true`
/// performs every read needed to classify each row as create/update/unchanged, but calls no store
/// write method at all — see the module doc for why that (not a rolled-back transaction) is what
/// makes `--dry-run` write nothing.
pub async fn import(
    stores: &Stores,
    pack: &Pack,
    dry_run: bool,
    owner_id: Uuid,
) -> Result<ImportReport, ImportError> {
    let mut report = ImportReport::default();

    for (slug_str, svc) in &pack.services {
        let slug = convert::parse_slug(slug_str)?;
        let model = convert::service_from_pack(owner_id, slug.clone(), svc)?;
        let existing = stores.service().get_by_slug(owner_id, &slug).await?;
        let change = match &existing {
            None => ImportChange::Created,
            Some(row) if row == &model => ImportChange::Unchanged,
            Some(_) => ImportChange::Updated,
        };
        if !dry_run {
            match existing {
                None => stores.service().create(&model).await?,
                Some(_) => stores.service().update(&model).await?,
            }
        }
        report.services.push((slug_str.clone(), change));
    }

    for (slug_str, provider) in &pack.auth_providers {
        let slug = convert::parse_slug(slug_str)?;
        let service_slug = convert::parse_slug(&provider.service)?;
        let model = convert::auth_provider_from_pack(
            owner_id,
            slug.clone(),
            service_slug.clone(),
            provider,
        )?;
        let existing = stores
            .auth_provider()
            .get(owner_id, &service_slug, &slug)
            .await?;
        let change = match &existing {
            None => ImportChange::Created,
            Some(row) if row == &model => ImportChange::Unchanged,
            Some(_) => ImportChange::Updated,
        };
        if !dry_run {
            match existing {
                None => stores.auth_provider().create(&model).await?,
                Some(_) => stores.auth_provider().update(&model).await?,
            }
        }
        report.auth_providers.push((slug_str.clone(), change));
    }

    for (slug_str, call) in &pack.api_calls {
        let slug = convert::parse_slug(slug_str)?;
        let service_slug = convert::parse_slug(&call.service)?;
        let auth_provider_slug = call
            .auth_provider
            .as_deref()
            .map(convert::parse_slug)
            .transpose()?;
        let model = convert::api_call_from_pack(
            owner_id,
            slug.clone(),
            service_slug.clone(),
            auth_provider_slug,
            call,
        )?;
        let tags = convert::tags_from_pack(&call.tags)?;
        let existing = stores
            .api_call()
            .get(owner_id, &service_slug, &slug)
            .await?;
        let change = match &existing {
            None => ImportChange::Created,
            Some(row) if row.api_call == model && row.tags == tags => ImportChange::Unchanged,
            Some(_) => ImportChange::Updated,
        };
        if !dry_run {
            match existing {
                None => stores.api_call().create(&model, &tags).await?,
                Some(_) => stores.api_call().update(&model, &tags).await?,
            }
        }
        report.api_calls.push((slug_str.clone(), change));
    }

    for (slug_str, script) in &pack.scripts {
        let slug = convert::parse_slug(slug_str)?;
        let model = convert::script_from_pack(owner_id, slug.clone(), script)?;
        let tags = convert::tags_from_pack(&script.tags)?;
        let existing = stores.script().get(owner_id, &slug).await?;
        let change = match &existing {
            None => ImportChange::Created,
            Some(row) if row.script == model && row.tags == tags => ImportChange::Unchanged,
            Some(_) => ImportChange::Updated,
        };
        if !dry_run {
            match existing {
                None => stores.script().create(&model, &tags).await?,
                Some(_) => stores.script().update(&model, &tags).await?,
            }
        }
        report.scripts.push((slug_str.clone(), change));
    }

    for (slug_str, endpoint) in &pack.endpoints {
        let slug = convert::parse_slug(slug_str)?;
        let model = convert::endpoint_from_pack(owner_id, slug.clone(), endpoint)?;
        let existing = stores.endpoint().get(owner_id, &slug).await?;
        let change = match &existing {
            None => ImportChange::Created,
            Some(row) if row == &model => ImportChange::Unchanged,
            Some(_) => ImportChange::Updated,
        };
        if !dry_run {
            match existing {
                None => stores.endpoint().create(&model).await?,
                Some(_) => stores.endpoint().update(&model).await?,
            }
        }
        report.endpoints.push((slug_str.clone(), change));
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::pack::{PackApiCall, PackEndpoint, PackPagination, PackService};
    use crate::store::test_support::ScratchDb;

    fn service(name: &str) -> PackService {
        PackService {
            base_url: format!("https://{name}.example.com/"),
            origin_allowlist: BTreeSet::from([format!("https://{name}.example.com")]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn basic_pack() -> Pack {
        let mut pack = Pack {
            version: super::super::PACK_VERSION,
            services: BTreeMap::from([("svc-import".to_owned(), service("svc-import"))]),
            auth_providers: BTreeMap::new(),
            api_calls: BTreeMap::from([(
                "call-import".to_owned(),
                PackApiCall {
                    service: "svc-import".to_owned(),
                    auth_provider: None,
                    method: "GET".to_owned(),
                    path_template: "/things".to_owned(),
                    query_fixed: BTreeMap::new(),
                    body_template: None,
                    access: "read".to_owned(),
                    idempotent: true,
                    projection: None,
                    pagination: PackPagination::None,
                    timeout_ms: None,
                    max_response_bytes: None,
                    params: Vec::new(),
                    tags: BTreeSet::from(["expose".to_owned()]),
                    description: Some("Fetch every thing.".to_owned()),
                },
            )]),
            scripts: BTreeMap::new(),
            endpoints: BTreeMap::from([(
                "ep-import".to_owned(),
                PackEndpoint {
                    tag_expr: "has(expose)".to_owned(),
                    write_ceiling: "read".to_owned(),
                    budgets: Default::default(),
                    instructions: None,
                    enabled: true,
                    aliases: BTreeMap::new(),
                    auth_providers: BTreeSet::new(),
                },
            )]),
            tags: BTreeSet::from(["expose".to_owned()]),
        };
        // Keep `validate`'s tag-vocabulary check happy without pulling it into every test here.
        pack.tags = BTreeSet::from(["expose".to_owned()]);
        pack
    }

    #[tokio::test]
    async fn dry_run_classifies_but_writes_nothing() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let pack = basic_pack();

        let report = import(&stores, &pack, true, owner_id).await.unwrap();
        assert_eq!(
            report.services,
            vec![("svc-import".to_owned(), ImportChange::Created)]
        );
        assert_eq!(
            report.api_calls,
            vec![("call-import".to_owned(), ImportChange::Created)]
        );

        assert!(
            stores
                .service()
                .get_by_slug(owner_id, &"svc-import".parse().unwrap())
                .await
                .unwrap()
                .is_none()
        );

        db.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn a_real_import_is_idempotent() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let pack = basic_pack();

        let first = import(&stores, &pack, false, owner_id).await.unwrap();
        assert!(!first.is_idempotent_no_op());

        let second = import(&stores, &pack, false, owner_id).await.unwrap();
        assert!(
            second.is_idempotent_no_op(),
            "re-import must classify everything Unchanged"
        );

        db.teardown().await.unwrap();
    }
}
