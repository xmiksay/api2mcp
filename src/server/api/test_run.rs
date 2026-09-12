//! Shared plumbing for `POST /api/api_calls/{slug}/test` and `POST /api/scripts/{slug}/test` —
//! the part of the admin API that makes a definition *editable*: run it for real, against the
//! real upstream, and show what came back next to what the model would actually see.
//!
//! The api_call path calls [`crate::runtime::Executor::run_tool`] directly, unmodified — budgets,
//! the SSRF guard, redaction and the audit record all apply exactly as they do for a real MCP
//! `tools/call`. Its return value only ever carries the *projected* result, though, so
//! [`run_api_call_test`] re-reads the run's own persisted `run_calls` row afterwards to recover
//! the raw upstream response body — see that function's doc.
//!
//! The script path cannot go through `Executor::run_tool` unmodified: a script failure's line,
//! column and source snippet ([`ScriptFailure`], `script::errors`) are exactly what
//! `Executor::run_tool` discards — its `Err(e)` arm folds a script failure down to `e.to_string()`
//! (`RunScriptError`'s `Display`, which never repeats structural detail already carried in typed
//! fields) before handing back a `RunResult`, whose `error` field is a plain `Option<String>`
//! shared by every caller of `run_tool` — widening it to carry a structured [`ScriptFailure`]
//! would change that shape for all of them, not just this route. [`run_script_test`] instead
//! calls the same `pub` building blocks
//! `Executor::run_tool` itself is built from — `budget::BudgetMeter`, `dispatch::AuthProviders`,
//! `fanout::ConcurrencyLimits`, `recorder::record` — so it is the identical pipeline (same
//! budgets, same auth loading, same audit write), just assembled here instead of inside
//! `Executor`, specifically so the structured [`ScriptFailure`] survives long enough to reach the
//! response body.
//!
//! Both paths persist exactly one `runs` row, same as any other tool call — a test run is a real
//! run.

use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::model::Slug;
use crate::resolve::plan::{PlannedTool, ToolTarget};
use crate::resolve::{EndpointPlan, ResolveError};
use crate::runtime::budget::BudgetMeter;
use crate::runtime::dispatch::{AuthProviders, DispatchContext};
use crate::runtime::fanout::ConcurrencyLimits;
use crate::runtime::recorder::{RunRecord, record};
use crate::runtime::{Executor, RunStatusView};
use crate::script::{RunScriptError, ScriptFailure};
use crate::server::state::AppState;
use crate::store::{RunCallerKind, RunStatus, Stores};

use super::{ApiError, Caller};

/// See this module's doc: a test run is attributed to the admin session that triggered it, but
/// `runs.caller_kind` is a DB `CHECK` restricted to `oauth`/`service_token`
/// (`migration::m0006_runs`), so recording a session-triggered test run under either value needs
/// a new append-only migration to widen that constraint, which hasn't happened.
/// `ServiceToken` is the closer fit of the two, and the `admin-test:` prefix on `caller_id` keeps
/// a test run visually distinct from a real one in the audit trail.
fn test_caller_kind() -> RunCallerKind {
    RunCallerKind::ServiceToken
}

fn test_caller_id(caller: &Caller) -> String {
    format!("admin-test:{}", caller.id)
}

fn to_status_view(status: RunStatus) -> RunStatusView {
    match status {
        RunStatus::Ok => RunStatusView::Ok,
        RunStatus::Partial => RunStatusView::Partial,
        RunStatus::Error => RunStatusView::Error,
        RunStatus::Denied => RunStatusView::Denied,
        RunStatus::BudgetExceeded => RunStatusView::BudgetExceeded,
        RunStatus::Timeout => RunStatusView::Timeout,
    }
}

pub async fn resolve_plan(
    state: &AppState,
    owner_id: Uuid,
    endpoint_slug: &Slug,
) -> Result<Arc<EndpointPlan>, ApiError> {
    state
        .plans
        .get_or_build(&state.stores(), owner_id, endpoint_slug)
        .await
        .map_err(|e| match e {
            ResolveError::EndpointNotFound { .. } => ApiError::NotFound(e.to_string()),
            other => ApiError::BadRequest(other.to_string()),
        })
}

/// Finds the tool this endpoint exposes for the api_call/script named `target_slug`. The test
/// routes address a definition by its own slug, but a plan's tools are looked up by *tool name*
/// (`EndpointPlan::tool`), which can differ from the slug under an `endpoint_aliases` rename.
pub fn find_tool<'a>(
    plan: &'a EndpointPlan,
    want_script: bool,
    target_slug: &Slug,
) -> Option<&'a PlannedTool> {
    plan.tools.iter().find(|t| match &t.target {
        ToolTarget::ApiCall(s) => !want_script && s == target_slug,
        ToolTarget::Script(s) => want_script && s == target_slug,
    })
}

#[derive(Debug, Serialize)]
pub struct ApiCallTestResult {
    pub run_id: Uuid,
    pub status: RunStatusView,
    /// The value after the api_call's own projection (if any) was applied — what a real MCP
    /// caller would see.
    pub projected: Option<Value>,
    /// The upstream's own response body for this call, straight from the audit row —
    /// unprojected, so an author can see exactly what a projection kept or dropped.
    pub raw: Option<Value>,
    pub error: Option<String>,
}

/// `POST /api/api_calls/{slug}/test`. See this module's doc for why `raw` is a second read
/// (`store::RunStore::get`) rather than something `Executor::run_tool`'s own return value
/// already carries.
pub async fn run_api_call_test(
    state: &AppState,
    endpoint_slug: &Slug,
    api_call_slug: &Slug,
    args: Value,
    caller: &Caller,
) -> Result<ApiCallTestResult, ApiError> {
    let plan = resolve_plan(state, caller.id, endpoint_slug).await?;
    let tool = find_tool(&plan, false, api_call_slug).ok_or_else(|| {
        ApiError::NotFound(format!(
            "api_call {:?} is not exposed by endpoint {:?}",
            api_call_slug.as_str(),
            endpoint_slug.as_str()
        ))
    })?;

    let executor = Executor::new(state.stores(), state.upstream.clone(), state.ssrf_policy());
    let result = executor
        .run_tool(
            &plan,
            &tool.name,
            args,
            test_caller_kind(),
            test_caller_id(caller),
        )
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let raw = last_run_call(&state.stores(), caller.id, result.run_id)
        .await?
        .and_then(|c| c.response_body);

    Ok(ApiCallTestResult {
        run_id: result.run_id,
        status: result.status,
        projected: result.value,
        raw,
        error: result.error,
    })
}

async fn last_run_call(
    stores: &Stores,
    owner_id: Uuid,
    run_id: Uuid,
) -> Result<Option<crate::store::RunCall>, ApiError> {
    let found = stores
        .run()
        .get(owner_id, run_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(found.and_then(|(_, calls)| calls.into_iter().next_back()))
}

#[derive(Debug, Serialize)]
pub struct ScriptCallView {
    pub seq: i32,
    pub api_call_slug: String,
    pub service_slug: String,
    pub status_code: Option<u16>,
    pub response_bytes: Option<u64>,
    pub raw: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ScriptTestResult {
    pub run_id: Uuid,
    pub status: RunStatusView,
    /// The script's own returned value — `None` exactly when `failure` is `Some`.
    pub value: Option<Value>,
    /// Every upstream call the script made, in input order (`run_calls.seq`).
    pub calls: Vec<ScriptCallView>,
    /// Present on failure: kind, message, and — when the engine had a `rhai::Position` for it —
    /// line, column and a source snippet with a caret. See `script::errors::ScriptFailure`.
    pub failure: Option<ScriptFailure>,
}

/// `POST /api/scripts/{slug}/test`. See this module's doc for why this assembles the same
/// budget/auth/concurrency/recorder pieces `Executor::run_tool` uses internally, rather than
/// calling `Executor::run_tool` itself: only this path lets a failure keep its structured
/// [`ScriptFailure`] instead of collapsing to a display string.
pub async fn run_script_test(
    state: &AppState,
    endpoint_slug: &Slug,
    script_slug: &Slug,
    args: Value,
    caller: &Caller,
) -> Result<ScriptTestResult, ApiError> {
    let plan = resolve_plan(state, caller.id, endpoint_slug).await?;
    let tool = find_tool(&plan, true, script_slug).ok_or_else(|| {
        ApiError::NotFound(format!(
            "script {:?} is not exposed by endpoint {:?}",
            script_slug.as_str(),
            endpoint_slug.as_str()
        ))
    })?;
    let script = plan.scripts.get(script_slug).ok_or(ApiError::Internal)?;

    let stores = state.stores();
    let started = Instant::now();
    let auth_provider_store = stores.auth_provider();
    let auth = AuthProviders::load(&auth_provider_store, &plan)
        .await
        .map_err(ApiError::from_store)?;
    let policy = state.ssrf_policy();
    let ctx = DispatchContext {
        pool: &state.upstream,
        policy: &policy,
        auth: &auth,
        // Mirrors `Executor::new`'s own default (`runtime/mod.rs`) — no separate opinion here.
        max_redirects: 5,
    };
    let limits = ConcurrencyLimits::build(&plan, tool.budgets.max_concurrency);
    let meter = BudgetMeter::new(tool.budgets);

    let outcome = crate::script::run_script(
        &plan,
        &ctx,
        &limits,
        &meter,
        script_slug,
        script,
        args.clone(),
    )
    .await;

    let (status, output_redacted, entries) = match &outcome {
        Ok(run) => (RunStatus::Ok, Some(run.value.clone()), run.calls.clone()),
        Err(_) => (RunStatus::Error, None, Vec::new()),
    };

    let run_id = record(
        &stores,
        RunRecord {
            plan: &plan,
            tool,
            caller_kind: test_caller_kind(),
            caller_id: test_caller_id(caller),
            request_id: Uuid::new_v4().to_string(),
            args,
            output_redacted,
            status,
            entries: &entries,
            meter: &meter,
            elapsed: Some(started.elapsed()),
        },
    )
    .await
    .map_err(ApiError::from_store)?;

    let calls = script_call_views(&stores, caller.id, run_id).await?;
    let status_view = to_status_view(status);

    Ok(match outcome {
        Ok(run) => ScriptTestResult {
            run_id,
            status: status_view,
            value: Some(run.value),
            calls,
            failure: None,
        },
        Err(err) => ScriptTestResult {
            run_id,
            status: status_view,
            value: None,
            calls,
            failure: Some(to_script_failure(err)),
        },
    })
}

fn to_script_failure(err: RunScriptError) -> ScriptFailure {
    match err {
        RunScriptError::Args(e) => ScriptFailure::runtime(format!("arguments: {e}")),
        RunScriptError::Script(f) => f,
    }
}

async fn script_call_views(
    stores: &Stores,
    owner_id: Uuid,
    run_id: Uuid,
) -> Result<Vec<ScriptCallView>, ApiError> {
    let found = stores
        .run()
        .get(owner_id, run_id)
        .await
        .map_err(ApiError::from_store)?;
    let calls = found.map(|(_, calls)| calls).unwrap_or_default();
    Ok(calls
        .into_iter()
        .map(|c| ScriptCallView {
            seq: c.seq,
            api_call_slug: c.api_call_slug.as_str().to_owned(),
            service_slug: c.service_slug.as_str().to_owned(),
            status_code: c.status_code,
            response_bytes: c.response_bytes,
            raw: c.response_body,
        })
        .collect())
}
