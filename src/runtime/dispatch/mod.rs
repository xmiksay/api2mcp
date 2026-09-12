//! [`dispatch`] is **the only path in the crate that can reach
//! `http::send`** (via `http::paginate`, which calls it). Every other way of getting from a name
//! to an HTTP request — a script's `api()`/`api_many()`, a directly-invoked tool, the CLI —
//! must funnel through this function.
//!
//! Name resolution has two shapes:
//! - `caller_script = Some(script)`: `name` is the alias a script used inside `api()`/`api_many()`,
//!   resolved through `plan.callable_by[script]` — a name absent from that map can never reach
//!   HTTP, whether because the script never declared it or because this endpoint doesn't expose
//!   it (I1's data structure is `EndpointPlan::callable_by` itself; this module only reads it).
//! - `caller_script = None`: the caller is a directly-invoked tool, so `name` is resolved against
//!   the plan's own tool set (`EndpointPlan::tool`), which additionally applies endpoint aliasing.
//!   A `Script`-target tool can't be dispatched directly — running a script is
//!   [`crate::script::run_script`]'s job, not this function's.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::http::{CallResponse, SendParams, SsrfPolicy, UpstreamPool};
use crate::model::{AuthProvider, Slug};
use crate::project::CompiledProjection;
use crate::resolve::EndpointPlan;
use crate::resolve::plan::{PlannedApiCall, ToolTarget};
use crate::schema::bind_args;
use crate::store::{AuthProviderStore, StoreError};

mod error;
pub use error::{DispatchAttempt, DispatchError};

/// Every non-secret [`AuthProvider`] a plan's selected api_calls might need, resolved once per
/// run (see [`AuthProviders::load`]) rather than hit the database on every dispatched call.
/// `dispatch` itself does no store I/O — this is the seam that keeps it that way.
#[derive(Debug, Default, Clone)]
pub struct AuthProviders(BTreeMap<Slug, AuthProvider>);

impl AuthProviders {
    /// Loads exactly the providers named by `plan.calls`' `auth_provider_slug`s, deduplicated by
    /// provider slug (a provider shared by several api_calls on the same service is fetched once).
    pub async fn load(
        auth_providers: &AuthProviderStore,
        plan: &EndpointPlan,
    ) -> Result<Self, StoreError> {
        let mut map = BTreeMap::new();
        for planned in plan.calls.values() {
            let Some(provider_slug) = &planned.api_call.auth_provider_slug else {
                continue;
            };
            if map.contains_key(provider_slug) {
                continue;
            }
            if let Some(provider) = auth_providers
                .get(plan.owner_id, &planned.service.slug, provider_slug)
                .await?
            {
                map.insert(provider_slug.clone(), provider);
            }
        }
        Ok(Self(map))
    }

    pub fn get(&self, slug: &Slug) -> Option<&AuthProvider> {
        self.0.get(slug)
    }
}

/// Static context shared by every dispatch in a run — built once, reused across every call in
/// every batch.
pub struct DispatchContext<'a> {
    pub pool: &'a UpstreamPool,
    pub policy: &'a SsrfPolicy,
    pub auth: &'a AuthProviders,
    pub max_redirects: u8,
}

/// The per-call knobs that come from `runtime::budget::BudgetMeter` and are — per the plan —
/// computed **once per batch** and shared by every call the batch dispatches, never recomputed
/// per call: that's what makes a byte/page/time race between concurrent calls impossible instead
/// of merely unlikely.
#[derive(Debug, Clone, Copy)]
pub struct CallBudget {
    pub max_response_bytes: u64,
    pub max_pages: u32,
    pub deadline: Duration,
}

/// One dispatched call's result: every page `http::paginate` fetched (in fetch order — pagination
/// is inherently sequential, so this is already deterministic without any fan-out machinery), and
/// the value the caller sees after projection.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub api_call_slug: Slug,
    pub service_slug: Slug,
    pub method: ::http::Method,
    /// The first (only, for a non-paginated call) request's headers/body as bound by
    /// `http::bind` — credential-free by construction: `bind` never sees an `AuthProvider`, only
    /// `http::send` applies one, later and separately. Safe for `runtime::recorder` to redact and
    /// persist verbatim as the audit trail of what was actually sent.
    pub request_headers: BTreeMap<String, String>,
    pub request_body: Option<Value>,
    pub pages: Vec<CallResponse>,
    pub value: Value,
}

impl DispatchOutcome {
    /// Total response bytes across every page, counted **after decompression, before
    /// projection** — the plan's own wording for what a byte budget measures.
    pub fn bytes_in(&self) -> u64 {
        self.pages.iter().map(|p| p.body.len() as u64).sum()
    }

    pub fn pages_fetched(&self) -> u32 {
        self.pages.len() as u32
    }
}

/// Resolves `name` to the [`PlannedApiCall`] it names, per the module docs. Pure (no I/O) — the
/// half of [`dispatch`] that's cheap to unit test without a network.
pub fn resolve<'a>(
    plan: &'a EndpointPlan,
    caller_script: Option<&Slug>,
    name: &str,
) -> Result<&'a PlannedApiCall, DispatchError> {
    let not_declared = || DispatchError::NotDeclared {
        name: name.to_owned(),
    };
    match caller_script {
        Some(script_slug) => {
            let reachable = plan.callable_by.get(script_slug).ok_or_else(not_declared)?;
            let api_call_slug = reachable.get(name).ok_or_else(not_declared)?;
            plan.calls.get(api_call_slug).ok_or_else(not_declared)
        }
        None => {
            let tool = plan.tool(name).ok_or_else(not_declared)?;
            match &tool.target {
                ToolTarget::ApiCall(slug) => plan.calls.get(slug).ok_or_else(not_declared),
                ToolTarget::Script(_) => Err(DispatchError::NotAnApiCall {
                    name: name.to_owned(),
                }),
            }
        }
    }
}

/// Resolves, binds, sends and projects one call. See the module docs for what makes this the
/// crate's sole route to `http::send`, and [`CallBudget`] for why its fields are computed once
/// per batch rather than passed fresh per call.
pub async fn dispatch(
    plan: &EndpointPlan,
    ctx: &DispatchContext<'_>,
    caller_script: Option<&Slug>,
    name: &str,
    args: &Value,
    budget: CallBudget,
) -> Result<DispatchOutcome, DispatchError> {
    let planned = resolve(plan, caller_script, name)?;

    let bound_args = bind_args(&planned.api_call.params, args)?;
    let request = crate::http::bind(&planned.api_call, &planned.service, &bound_args)?;
    let request_headers = request.headers.clone();
    let request_body = request.body.clone();

    let client = ctx
        .pool
        .client_for(&planned.service)
        .await
        .map_err(|e| DispatchError::Client(e.to_string()))?;

    let auth = planned
        .api_call
        .auth_provider_slug
        .as_ref()
        .and_then(|slug| ctx.auth.get(slug));

    let static_cap = planned
        .api_call
        .max_response_bytes
        .unwrap_or(planned.service.max_response_bytes);
    let send_params = SendParams {
        allowlist: &planned.service.origin_allowlist,
        policy: ctx.policy,
        auth,
        max_response_bytes: static_cap.min(budget.max_response_bytes),
        deadline: budget.deadline,
        max_redirects: ctx.max_redirects,
    };

    let pages = crate::http::paginate(
        &client,
        request,
        &planned.api_call.pagination,
        budget.max_pages,
        send_params,
    )
    .await?;

    // A non-2xx upstream response is a failure, not a value. Projecting an error body and
    // handing it back as a successful result is the worst outcome available: the model cannot
    // tell it apart from real data, and the projection will usually "succeed" on it — an error
    // payload is still JSON. This is the plan's `kind: "http_status"` per-item failure.
    //
    // From here on, `pages` is a real, completed upstream exchange — every remaining failure
    // path attaches it as a `DispatchAttempt` so `runtime::recorder` can still write a truthful
    // `run_calls` row even though the call as a whole didn't succeed (see `DispatchAttempt`'s own
    // doc for why that lives on the error rather than reshaping `ItemOutcome`).
    if let Some(bad) = pages.iter().find(|p| !p.status.is_success()) {
        let error = http_status_error(bad);
        // `bad` borrows `pages`; its borrow ends with the statement above, so `pages` can move.
        let attempt = DispatchAttempt {
            api_call_slug: planned.api_call.slug.clone(),
            service_slug: planned.service.slug.clone(),
            method: planned.api_call.method.clone(),
            request_headers,
            request_body,
            pages,
        };
        return Err(error.with_attempt(attempt));
    }
    let value = match project_pages(planned, &pages) {
        Ok(value) => value,
        Err(e) => {
            let attempt = DispatchAttempt {
                api_call_slug: planned.api_call.slug.clone(),
                service_slug: planned.service.slug.clone(),
                method: planned.api_call.method.clone(),
                request_headers,
                request_body,
                pages,
            };
            return Err(e.with_attempt(attempt));
        }
    };

    Ok(DispatchOutcome {
        api_call_slug: planned.api_call.slug.clone(),
        service_slug: planned.service.slug.clone(),
        method: planned.api_call.method.clone(),
        request_headers,
        request_body,
        pages,
        value,
    })
}

/// Applies the api_call's projection (if any) to each page's body independently and in fetch
/// order (I7 — pagination is already sequential, so this adds no reordering risk of its own),
/// then collapses to a single value for the common one-page case or an array of per-page values
/// otherwise.
///
/// Recompiles the projection from `planned.api_call.projection` (the raw, pre-compile
/// `model::Projection`) on every call rather than reusing `planned.projection`
/// (`resolve::plan::CompiledProjection`, already parsed once at plan-build time): that type's
/// fields are `pub`, but `crate::project::CompiledProjection` — the type `crate::project::apply`
/// actually takes — only exposes a private-field, `compile`-only constructor. This is a real
/// inefficiency (every dispatch re-parses every JSONPath expression in the projection instead of
/// reusing the copy already parsed at plan-build time); fixing it would need a `pub(crate)`
/// constructor (or field access) on `project::CompiledProjection` so this function could adapt
/// the already-parsed `resolve::plan::CompiledProjection` directly.
/// Builds the failure for a non-2xx page, carrying a bounded excerpt of the upstream's body.
/// Truncated because an upstream error page can be a megabyte of HTML, and redacted because it
/// is about to cross into a model's context and an audit row (I4).
fn http_status_error(page: &CallResponse) -> DispatchError {
    const DETAIL_CAP: usize = 512;
    let body = String::from_utf8_lossy(&page.body);
    let mut detail = crate::http::redact_message(body.trim());
    if detail.len() > DETAIL_CAP {
        detail.truncate(DETAIL_CAP);
        detail.push('\u{2026}');
    }
    DispatchError::HttpStatus {
        status: page.status.as_u16(),
        reason: page.status.canonical_reason().unwrap_or("").to_owned(),
        detail,
        attempt: None,
    }
}

fn project_pages(planned: &PlannedApiCall, pages: &[CallResponse]) -> Result<Value, DispatchError> {
    let compiled = planned
        .api_call
        .projection
        .as_ref()
        .map(CompiledProjection::compile)
        .transpose()
        .map_err(|source| DispatchError::Projection {
            source,
            attempt: None,
        })?;

    let mut values = Vec::with_capacity(pages.len());
    for page in pages {
        let body: Value = if page.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&page.body).map_err(|e| DispatchError::ResponseNotJson {
                message: e.to_string(),
                attempt: None,
            })?
        };
        let projected = match &compiled {
            Some(p) => {
                crate::project::apply(p, &body).map_err(|source| DispatchError::Projection {
                    source,
                    attempt: None,
                })?
            }
            None => body,
        };
        values.push(projected);
    }

    Ok(match values.len() {
        1 => values
            .into_iter()
            .next()
            .expect("len checked to be exactly 1"),
        _ => Value::Array(values),
    })
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
