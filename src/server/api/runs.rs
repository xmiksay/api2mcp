//! `GET /api/runs` (filtered, paged) and `GET /api/runs/{id}` with its `run_calls` — the audit
//! trail. Every field here is already redaction-safe: `RunSummary`/`RunCall` are the store's own
//! read shapes, built from columns `runtime::recorder` wrote through `http::redact` in the first
//! place (see that module's own doc); this layer only renders them as JSON.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::server::state::AppState;
use crate::store::{RunCall, RunCallerKind, RunFilter, RunStatus, RunSummary};

use super::convert::parse_slug;
use super::{ApiError, Caller};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/runs", get(list))
        .route("/runs/{id}", get(get_one))
}

pub(crate) const DEFAULT_LIMIT: u64 = 50;
pub(crate) const MAX_LIMIT: u64 = 500;

#[derive(Debug, Deserialize)]
struct RunsQuery {
    endpoint: Option<String>,
    status: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
}

pub(crate) fn parse_status(s: &str) -> Result<RunStatus, ApiError> {
    match s {
        "ok" => Ok(RunStatus::Ok),
        "partial" => Ok(RunStatus::Partial),
        "error" => Ok(RunStatus::Error),
        "denied" => Ok(RunStatus::Denied),
        "budget_exceeded" => Ok(RunStatus::BudgetExceeded),
        "timeout" => Ok(RunStatus::Timeout),
        other => Err(ApiError::BadRequest(format!(
            "status {other:?}: expected one of ok|partial|error|denied|budget_exceeded|timeout"
        ))),
    }
}

pub(crate) fn status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Ok => "ok",
        RunStatus::Partial => "partial",
        RunStatus::Error => "error",
        RunStatus::Denied => "denied",
        RunStatus::BudgetExceeded => "budget_exceeded",
        RunStatus::Timeout => "timeout",
    }
}

pub(crate) fn target_kind_str(k: crate::store::RunTargetKind) -> &'static str {
    match k {
        crate::store::RunTargetKind::ApiCall => "api_call",
        crate::store::RunTargetKind::Script => "script",
    }
}

pub(crate) fn caller_kind_str(k: RunCallerKind) -> &'static str {
    match k {
        RunCallerKind::Session => "session",
        RunCallerKind::Oauth => "oauth",
        RunCallerKind::ServiceToken => "service_token",
        RunCallerKind::Cli => "cli",
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct RunSummaryView {
    id: Uuid,
    endpoint_slug: String,
    tool_name: String,
    target_kind: &'static str,
    target_slug: String,
    status: &'static str,
    calls_made: u32,
    bytes_in: u64,
    pages_fetched: u32,
    created_at: String,
}

pub(crate) fn summary_view(s: &RunSummary) -> RunSummaryView {
    RunSummaryView {
        id: s.id,
        endpoint_slug: s.endpoint_slug.as_str().to_owned(),
        tool_name: s.tool_name.clone(),
        target_kind: target_kind_str(s.target_kind),
        target_slug: s.target_slug.as_str().to_owned(),
        status: status_str(s.status),
        calls_made: s.calls_made,
        bytes_in: s.bytes_in,
        pages_fetched: s.pages_fetched,
        created_at: s.created_at.to_rfc3339(),
    }
}

/// The real logic behind `list`, reused by `server::mcp::control::runs`'s `run.list` tool — see
/// `services.rs`'s own comment. Takes an already-built [`RunFilter`] rather than the wire query
/// shape: the control plane's own tool arguments aren't URL query params, so parsing them into a
/// `RunFilter` is that caller's job, not this function's.
pub(crate) async fn list_for_owner(
    state: &AppState,
    owner_id: Uuid,
    filter: &RunFilter,
) -> Result<Vec<RunSummaryView>, ApiError> {
    let runs = state
        .stores()
        .run()
        .list(owner_id, filter)
        .await
        .map_err(ApiError::from_store)?;
    Ok(runs.iter().map(summary_view).collect())
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Vec<RunSummaryView>>, ApiError> {
    let endpoint_slug = q
        .endpoint
        .as_deref()
        .map(parse_slug)
        .transpose()
        .map_err(ApiError::BadRequest)?;
    let status = q.status.as_deref().map(parse_status).transpose()?;
    let filter = RunFilter {
        endpoint_slug,
        status,
        limit: q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT),
        offset: q.offset.unwrap_or(0),
    };
    Ok(Json(list_for_owner(&state, caller.id, &filter).await?))
}

#[derive(Debug, Serialize)]
pub(crate) struct RunCallView {
    seq: i32,
    api_call_slug: String,
    service_slug: String,
    method: String,
    /// Already redacted at write time (`http::redact`) — see `RunDetailView`'s own doc: nothing
    /// here gets a second redaction pass.
    url_redacted: String,
    headers_redacted: Option<Value>,
    status_code: Option<u16>,
    response_bytes: Option<u64>,
    response_truncated: bool,
    error: Option<String>,
    /// The upstream's own response body, straight from the audit row — see `test_run`'s module
    /// doc for the same "raw vs. projected" reasoning applied to the run log's own history, not
    /// just a fresh test run.
    raw: Option<Value>,
}

pub(crate) fn call_view(c: &RunCall) -> RunCallView {
    RunCallView {
        seq: c.seq,
        api_call_slug: c.api_call_slug.as_str().to_owned(),
        service_slug: c.service_slug.as_str().to_owned(),
        method: c.method.to_string(),
        url_redacted: c.url_redacted.clone(),
        headers_redacted: c.headers_redacted.clone(),
        status_code: c.status_code,
        response_bytes: c.response_bytes,
        response_truncated: c.response_truncated,
        error: c.error.clone(),
        raw: c.response_body.clone(),
    }
}

/// The full audit record — everything `store::run::RunDetail` carries beyond
/// [`RunSummaryView`], redaction-safe by construction (every field here was already redacted at
/// write time by `runtime::recorder`/`http::redact`; this layer never re-redacts, it only renders
/// already-safe columns as JSON — re-redacting here would suggest the write-time pass can't be
/// trusted, which is exactly backwards).
///
/// `definition_snapshot` lives **only** here, never on [`RunSummaryView`]/the list route: it's
/// the full compiled api_call/script/service/budgets slice a tool ran from and can be large, and
/// a run listing has no use for it. Keep it that way — this asymmetry (everything else flat and
/// shared with the summary, this one field detail-only) is deliberate, not a gap to close later.
#[derive(Debug, Serialize)]
pub(crate) struct RunDetailView {
    #[serde(flatten)]
    summary: RunSummaryView,
    caller_kind: &'static str,
    caller_id: String,
    request_id: String,
    /// The run's frozen wall-clock start (`runtime::budget::BudgetMeter::execution_start`) — what
    /// makes a script built on `execution_start()` actually replayable from this record.
    execution_start: String,
    definition_snapshot: Value,
    definition_digest: String,
    input_redacted: Value,
    output_redacted: Option<Value>,
    errors: Option<Value>,
    budget_snapshot: Option<Value>,
    timings: Option<Value>,
    calls: Vec<RunCallView>,
}

/// The real logic behind `get_one`, reused by `server::mcp::control::runs`'s `run.get` tool —
/// see `services.rs`'s own comment.
pub(crate) async fn get_for_owner(
    state: &AppState,
    owner_id: Uuid,
    id: Uuid,
) -> Result<RunDetailView, ApiError> {
    let (detail, calls) = state
        .stores()
        .run()
        .get(owner_id, id)
        .await
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::NotFound(format!("run {id} not found")))?;
    Ok(RunDetailView {
        summary: summary_view(&detail.summary),
        caller_kind: caller_kind_str(detail.caller_kind),
        caller_id: detail.caller_id,
        request_id: detail.request_id,
        execution_start: detail.execution_start.to_rfc3339(),
        definition_snapshot: detail.definition_snapshot,
        definition_digest: detail.definition_digest,
        input_redacted: detail.input_redacted,
        output_redacted: detail.output_redacted,
        errors: detail.errors,
        budget_snapshot: detail.budget_snapshot,
        timings: detail.timings,
        calls: calls.iter().map(call_view).collect(),
    })
}

async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<Json<RunDetailView>, ApiError> {
    Ok(Json(get_for_owner(&state, caller.id, id).await?))
}
