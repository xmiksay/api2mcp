//! I7's concurrent half: [`fan_out`] runs a batch of futures concurrently while keeping the
//! *result* deterministic — completion order must never be observable, only input order is.
//!
//! Every future is tagged with its own input index and written into a pre-sized `Vec<Option<T>>`
//! slot as it completes; the final `Vec<T>` is built by walking that vector 0..len, never by the
//! order results arrived in. [`futures::stream::FuturesUnordered`] is used deliberately —
//! `buffered`/`buffer_unordered` impose their *own* notion of ordering (buffered preserves input
//! order at the cost of head-of-line blocking; buffer_unordered doesn't even guarantee draining
//! order), which would make this module's determinism a property of a combinator's internals
//! rather than of the slot-assignment code actually written here.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

use crate::model::Slug;
use crate::resolve::EndpointPlan;

/// A boxed future — what a caller building a batch of dispatch calls hands to [`fan_out`]. Carries
/// an explicit lifetime (rather than defaulting to `'static`) because a real batch's futures
/// borrow the plan, the dispatch context and the budget meter for the duration of the call —
/// all of which outlive the batch itself but none of which is `'static`.
pub type BoxedCall<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Per-service concurrency semaphores, built once per run from the plan's reachable services
/// (never per batch, never per call) — the plan's own wording. Bounded by the lower of the
/// service's own `max_concurrency` and the run's folded `Budgets::max_concurrency`, so neither a
/// generous per-service default nor a permissive tool budget alone can widen the other.
pub struct ConcurrencyLimits {
    by_service: std::collections::BTreeMap<Slug, Arc<Semaphore>>,
}

impl ConcurrencyLimits {
    pub fn build(plan: &EndpointPlan, budget_max_concurrency: Option<u32>) -> Self {
        let mut by_service = std::collections::BTreeMap::new();
        for planned in plan.calls.values() {
            by_service
                .entry(planned.service.slug.clone())
                .or_insert_with(|| {
                    let ceiling = match budget_max_concurrency {
                        Some(b) => planned.service.max_concurrency.min(b),
                        None => planned.service.max_concurrency,
                    };
                    Arc::new(Semaphore::new(ceiling.max(1) as usize))
                });
        }
        Self { by_service }
    }

    fn semaphore_for(&self, service_slug: &Slug) -> Option<Arc<Semaphore>> {
        self.by_service.get(service_slug).cloned()
    }
}

/// Runs `items` concurrently, each bounded by its own service's semaphore, and returns their
/// results in **input order** — see the module docs for why completion order never leaks through.
///
/// `items` is `(service_slug, future)` pairs; a service with no registered semaphore (shouldn't
/// happen for anything `dispatch` actually resolved, since [`ConcurrencyLimits::build`] covers
/// every service in `plan.calls`) runs unbounded rather than panicking — a defensive fallback,
/// not a path any real batch should take.
pub async fn fan_out<'a, T: Send + 'a>(
    limits: &ConcurrencyLimits,
    items: Vec<(Slug, BoxedCall<'a, T>)>,
) -> Vec<T> {
    let len = items.len();
    let mut unordered = FuturesUnordered::new();

    for (index, (service_slug, call)) in items.into_iter().enumerate() {
        let permit = limits.semaphore_for(&service_slug);
        unordered.push(async move {
            let _permit = match permit {
                Some(sem) => Some(
                    sem.acquire_owned()
                        .await
                        .expect("semaphore is never closed for the lifetime of a run"),
                ),
                None => None,
            };
            (index, call.await)
        });
    }

    let mut slots: Vec<Option<T>> = (0..len).map(|_| None).collect();
    while let Some((index, value)) = unordered.next().await {
        slots[index] = Some(value);
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("fan_out fills every input index exactly once"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::model::{Access, ApiCall, Budgets, Origin, Pagination, Service};
    use crate::resolve::plan::PlannedApiCall;
    use std::collections::{BTreeMap, BTreeSet};

    fn service(slug: &str, max_concurrency: u32) -> Service {
        let base_url: url::Url = format!("https://{slug}.example.com/").parse().unwrap();
        Service {
            owner_id: uuid::Uuid::nil(),
            slug: slug.parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn plan_with_one_service(max_concurrency: u32) -> EndpointPlan {
        let svc = service("svc", max_concurrency);
        let call = ApiCall {
            owner_id: uuid::Uuid::nil(),
            slug: "call-a".parse().unwrap(),
            service_slug: svc.slug.clone(),
            method: http::Method::GET,
            path_template: "/things".to_owned(),
            query_fixed: BTreeMap::new(),
            body_template: None,
            access: Access::Read,
            idempotent: true,
            projection: None,
            pagination: Pagination::None,
            timeout_ms: None,
            max_response_bytes: None,
            params: vec![],
            description: None,
        };
        let planned = PlannedApiCall {
            url_template: crate::http::UrlTemplate::parse(&call.path_template).unwrap(),
            origin: Origin::of(&svc.base_url).unwrap(),
            api_call: call.clone(),
            service: svc,
            projection: None,
        };
        let mut calls = BTreeMap::new();
        calls.insert(call.slug.clone(), planned);
        EndpointPlan {
            owner_id: uuid::Uuid::nil(),
            slug: "ep".parse().unwrap(),
            write_ceiling: Access::Read,
            instructions: None,
            tools: vec![],
            calls,
            scripts: BTreeMap::new(),
            callable_by: BTreeMap::new(),
            origins: BTreeSet::new(),
            budgets: Budgets::default(),
            digest: "digest".to_owned(),
        }
    }

    #[tokio::test]
    async fn results_land_in_input_order_regardless_of_completion_order() {
        let plan = plan_with_one_service(10);
        let limits = ConcurrencyLimits::build(&plan, None);

        // Item 0 finishes last, item 4 finishes first — fan_out must still return 0..5 in order.
        let items: Vec<(Slug, BoxedCall<'static, usize>)> = (0..5)
            .map(|i| {
                let delay = Duration::from_millis((5 - i) * 10);
                let fut: BoxedCall<'static, usize> = Box::pin(async move {
                    tokio::time::sleep(delay).await;
                    i as usize
                });
                ("svc".parse().unwrap(), fut)
            })
            .collect();

        let results = fan_out(&limits, items).await;
        assert_eq!(results, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn concurrency_never_exceeds_the_services_ceiling() {
        let plan = plan_with_one_service(2);
        let limits = ConcurrencyLimits::build(&plan, None);

        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let items: Vec<(Slug, BoxedCall<'static, ()>)> = (0..8)
            .map(|_| {
                let in_flight = Arc::clone(&in_flight);
                let max_seen = Arc::clone(&max_seen);
                let fut: BoxedCall<'static, ()> = Box::pin(async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
                ("svc".parse().unwrap(), fut)
            })
            .collect();

        fan_out(&limits, items).await;
        assert!(max_seen.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn budget_max_concurrency_narrows_the_services_own_ceiling() {
        let plan = plan_with_one_service(10);
        let limits = ConcurrencyLimits::build(&plan, Some(1));

        let max_seen = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let items: Vec<(Slug, BoxedCall<'static, ()>)> = (0..4)
            .map(|_| {
                let in_flight = Arc::clone(&in_flight);
                let max_seen = Arc::clone(&max_seen);
                let fut: BoxedCall<'static, ()> = Box::pin(async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
                ("svc".parse().unwrap(), fut)
            })
            .collect();

        fan_out(&limits, items).await;
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }
}
