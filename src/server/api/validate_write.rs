//! The pre-write gate every CRUD route in `server::api` funnels through, **except**
//! `auth_providers.rs`: build a [`Pack`] snapshot of *every* definition currently in the
//! database, apply the one pending change this request wants to make, and run
//! [`crate::pack::validate`] over the result — the same function `api2mcp import` runs, so a
//! hand-authored YAML pack and a form submission are held to identical rules (URL templates
//! compile, projections compile, tag expressions parse, a script's declared api_calls exist, a
//! service's own origin is in its allowlist — see that module's own doc for the full list).
//! Reporting *every* failure in one pass, not just the first, is the entire reason this
//! revalidates the whole definition set instead of just the one item being written: someone
//! fixing a form should see everything wrong with it at once.
//!
//! **Auth providers don't go through here** because a pack carries no auth providers at all (see
//! `pack`'s own module doc) — there is no `Pack::auth_providers` field left for a
//! `PendingChange` to edit or for [`crate::pack::validate`] to check. `auth_providers.rs` does
//! its own direct, live check instead (bound_origin against the service's own row, fetched from
//! the database, not reconstructed into a `Pack`).
//!
//! A useful side effect: because the snapshot always includes every *other* definition unchanged,
//! removing an entity (a `Remove*` variant) and revalidating catches a now-dangling reference
//! (a script whose declared api_call just disappeared, an endpoint alias pointing at nothing)
//! *before* the delete ever reaches the database, as a clean [`crate::server::error::ApiError::Validation`]
//! rather than a raw foreign-key violation.
//!
//! This module intentionally does not call [`crate::pack::convert`] — that module is private to
//! `pack::` (see `super::convert`'s own doc) — so building the snapshot uses this module's own
//! `to_pack` conversions instead.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::pack::{Pack, PackApiCall, PackEndpoint, PackScript, PackService};
use crate::store::{StoreError, Stores};

use super::convert::tags_to_pack;
use super::convert_items::{api_call_to_pack, endpoint_to_pack, script_to_pack};

/// One pending create/update/delete, expressed as the edit it makes to a full-database [`Pack`]
/// snapshot. A create and an update are the same variant (`Upsert*`) — both replace whatever the
/// pack's map holds at that slug, which is exactly "insert if absent, replace if present".
pub enum PendingChange {
    UpsertService(String, PackService),
    RemoveService(String),
    UpsertApiCall(String, PackApiCall),
    RemoveApiCall(String),
    UpsertScript(String, PackScript),
    RemoveScript(String),
    UpsertEndpoint(String, PackEndpoint),
    RemoveEndpoint(String),
}

/// Builds a [`Pack`] holding every definition `owner_id` owns — the "current state of the
/// world" a pending change is validated against. Scoped to one owner, not the whole database:
/// a user owns everything they create and can use only their own, so the snapshot a form
/// submission is checked against must never let another owner's slugs or references leak in.
pub async fn build_full_pack(stores: &Stores, owner_id: Uuid) -> Result<Pack, StoreError> {
    let services = stores.service().list(owner_id).await?;
    let mut services_map = BTreeMap::new();
    for svc in &services {
        services_map.insert(
            svc.slug.as_str().to_owned(),
            super::convert::service_to_pack(svc),
        );
    }

    let tagged_calls = stores.api_call().list_all(owner_id).await?;
    let tagged_scripts = stores.script().list_all(owner_id).await?;
    let endpoints = stores.endpoint().list_all(owner_id).await?;

    let mut api_calls = BTreeMap::new();
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for t in &tagged_calls {
        api_calls.insert(
            t.api_call.slug.as_str().to_owned(),
            api_call_to_pack(&t.api_call, &t.tags),
        );
        tags.extend(tags_to_pack(&t.tags));
    }

    let mut scripts = BTreeMap::new();
    for t in &tagged_scripts {
        scripts.insert(
            t.script.slug.as_str().to_owned(),
            script_to_pack(&t.script, &t.tags),
        );
        tags.extend(tags_to_pack(&t.tags));
    }

    let mut endpoints_map = BTreeMap::new();
    for e in &endpoints {
        endpoints_map.insert(e.slug.as_str().to_owned(), endpoint_to_pack(e));
    }

    Ok(Pack {
        version: crate::pack::PACK_VERSION,
        services: services_map,
        api_calls,
        scripts,
        endpoints: endpoints_map,
        tags,
    })
}

/// Applies `change` to `pack` in place, then recomputes `pack.tags` from the (possibly just
/// changed) union of every api_call's and script's own tags — `pack::validate`'s tag-vocabulary
/// check compares `pack.tags` against exactly that union, and this API always derives the
/// vocabulary rather than asking a caller to keep it in sync by hand.
fn apply_change(pack: &mut Pack, change: PendingChange) {
    match change {
        PendingChange::UpsertService(slug, s) => {
            pack.services.insert(slug, s);
        }
        PendingChange::RemoveService(slug) => {
            pack.services.remove(&slug);
        }
        PendingChange::UpsertApiCall(slug, c) => {
            pack.api_calls.insert(slug, c);
        }
        PendingChange::RemoveApiCall(slug) => {
            pack.api_calls.remove(&slug);
        }
        PendingChange::UpsertScript(slug, s) => {
            pack.scripts.insert(slug, s);
        }
        PendingChange::RemoveScript(slug) => {
            pack.scripts.remove(&slug);
        }
        PendingChange::UpsertEndpoint(slug, e) => {
            pack.endpoints.insert(slug, e);
        }
        PendingChange::RemoveEndpoint(slug) => {
            pack.endpoints.remove(&slug);
        }
    }
    let mut tags = BTreeSet::new();
    for c in pack.api_calls.values() {
        tags.extend(c.tags.iter().cloned());
    }
    for s in pack.scripts.values() {
        tags.extend(s.tags.iter().cloned());
    }
    pack.tags = tags;
}

/// Builds the full-database snapshot, applies `change`, and runs [`crate::pack::validate`] over
/// the result. `Ok(())` means the change is safe to persist; `Err` carries every failure found,
/// each already rendered to a message safe to return to a client (`pack::ValidationError`'s
/// `Display`, which never formats a credential — see that type's own tests).
pub async fn validate_change(
    stores: &Stores,
    owner_id: Uuid,
    change: PendingChange,
) -> Result<(), Vec<String>> {
    let mut pack = build_full_pack(stores, owner_id)
        .await
        .map_err(|e| vec![e.to_string()])?;
    apply_change(&mut pack, change);
    crate::pack::validate(&pack).map_err(|errors| errors.iter().map(|e| e.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::ScratchDb;
    use std::collections::BTreeMap as Map;

    fn service_body() -> PackService {
        PackService {
            base_url: "https://svc-validate-write.example.com/".to_owned(),
            origin_allowlist: BTreeSet::from(["https://svc-validate-write.example.com".to_owned()]),
            default_headers: Map::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    #[tokio::test]
    async fn a_valid_new_service_passes() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let result = validate_change(
            &stores,
            owner_id,
            PendingChange::UpsertService("svc-validate-write".to_owned(), service_body()),
        )
        .await;
        assert!(result.is_ok(), "unexpected errors: {result:?}");
        db.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn an_inconsistent_allowlist_is_rejected() {
        let Some(db) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(db.db.clone());
        let owner_id = db.create_user().await.unwrap();
        let mut body = service_body();
        body.origin_allowlist = BTreeSet::new();
        let result = validate_change(
            &stores,
            owner_id,
            PendingChange::UpsertService("svc-validate-write-bad".to_owned(), body),
        )
        .await;
        assert!(result.unwrap_err().iter().any(|e| e.contains("allowlist")));
        db.teardown().await.unwrap();
    }
}
