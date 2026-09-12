//! `PlanCache`: `(owner_id, slug) -> (generation, Arc<EndpointPlan>)`, invalidated the moment
//! `meta.definitions_generation` moves. A client calls `tools/list` on every session start;
//! without this, that would mean re-querying every definition table, recompiling every
//! template and regenerating every JSON Schema on every single request.
//!
//! Keyed by `(owner_id, slug)`, not `slug` alone: `endpoints.slug` is unique per owner, not
//! globally, so two different people's endpoints can share a slug and must never share a cache
//! entry.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::model::Slug;
use crate::store::Stores;

use super::plan::EndpointPlan;
use super::{ResolveError, build_plan};

type CacheKey = (Uuid, Slug);

#[derive(Default)]
pub struct PlanCache {
    // A plain `std::sync::Mutex` guarding only a map lookup/insert (never held across an
    // `.await`) is enough here — the expensive work (`build_plan`) runs outside the lock, so
    // there's no async-under-lock hazard and no need for `tokio::sync::Mutex`.
    entries: Mutex<BTreeMap<CacheKey, (u64, Arc<EndpointPlan>)>>,
}

impl PlanCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached plan for `(owner_id, slug)` if it's still current for the store's
    /// `meta.definitions_generation`, otherwise builds one, caches it, and returns that.
    pub async fn get_or_build(
        &self,
        stores: &Stores,
        owner_id: Uuid,
        slug: &Slug,
    ) -> Result<Arc<EndpointPlan>, ResolveError> {
        let current_gen = stores
            .meta()
            .definitions_generation()
            .await
            .map_err(|e| ResolveError::Store(e.to_string()))?;

        let key: CacheKey = (owner_id, slug.clone());
        if let Some(plan) = self.cached_at(&key, current_gen) {
            return Ok(plan);
        }

        let plan = Arc::new(build_plan(stores, owner_id, slug).await?);
        self.lock().insert(key, (current_gen, plan.clone()));
        Ok(plan)
    }

    fn cached_at(&self, key: &CacheKey, generation: u64) -> Option<Arc<EndpointPlan>> {
        self.lock()
            .get(key)
            .and_then(|(g, plan)| (*g == generation).then(|| plan.clone()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<CacheKey, (u64, Arc<EndpointPlan>)>> {
        // A prior panic while holding this lock would poison it; recovering the map rather
        // than propagating the panic here keeps one bad request from wedging the cache for
        // every subsequent one.
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Drops every cached entry, forcing the next [`Self::get_or_build`] call for any slug to
    /// rebuild — used by tests that want to force a rebuild without going through a real
    /// `meta.definitions_generation` bump.
    #[cfg(test)]
    pub fn clear(&self) {
        self.lock().clear();
    }
}
