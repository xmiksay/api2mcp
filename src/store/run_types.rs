//! Plain data shapes for `store::run` — split out purely to keep that file under the workspace's
//! 400-line cap (same reasoning as `runtime::dispatch`'s own `error.rs` split), not because these
//! types belong to a different concern: `RunStore` in `run.rs` is still their only reader/writer.

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::model::Slug;

use super::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTargetKind {
    ApiCall,
    Script,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCallerKind {
    Oauth,
    ServiceToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    Partial,
    Error,
    Denied,
    BudgetExceeded,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct NewRun {
    pub endpoint_slug: Slug,
    pub tool_name: String,
    pub target_kind: RunTargetKind,
    pub target_slug: Slug,
    pub caller_kind: RunCallerKind,
    pub caller_id: String,
    pub request_id: String,
    /// The run's frozen wall-clock start (`runtime::budget::BudgetMeter::execution_start`) — see
    /// that field's own doc for why it's the source of truth a reproducible script reads from.
    pub execution_start: DateTime<Utc>,
    pub definition_snapshot: Value,
    pub definition_digest: String,
    pub input_redacted: Value,
    pub output_redacted: Option<Value>,
    pub status: RunStatus,
    pub errors: Option<Value>,
    pub calls_made: u32,
    pub bytes_in: u64,
    pub pages_fetched: u32,
    pub budget_snapshot: Option<Value>,
    pub timings: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct NewRunCall {
    pub seq: i32,
    pub api_call_slug: Slug,
    pub service_slug: Slug,
    pub method: http::Method,
    pub url_redacted: String,
    pub headers_redacted: Option<Value>,
    pub body_redacted: Option<Value>,
    pub status_code: Option<u16>,
    pub response_bytes: Option<u64>,
    pub response_truncated: bool,
    pub error: Option<String>,
    pub timings: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub id: Uuid,
    pub endpoint_slug: Slug,
    pub tool_name: String,
    pub target_kind: RunTargetKind,
    pub target_slug: Slug,
    pub status: RunStatus,
    pub calls_made: u32,
    pub bytes_in: u64,
    pub pages_fetched: u32,
    pub created_at: DateTime<Utc>,
}

/// Everything [`RunSummary`] has, plus the fields only `GET /api/runs/{id}` — never the list
/// route — has any use for. `definition_snapshot` in particular can be large (the full compiled
/// api_call/script/service/budgets slice, per `runtime::snapshot`'s own doc): keeping it off
/// [`RunSummary`] and therefore off the list route is deliberate, not an oversight to "fix" by
/// merging the two back together.
#[derive(Debug, Clone)]
pub struct RunDetail {
    pub summary: RunSummary,
    pub caller_kind: RunCallerKind,
    pub caller_id: String,
    pub request_id: String,
    pub execution_start: DateTime<Utc>,
    pub definition_snapshot: Value,
    pub definition_digest: String,
    pub input_redacted: Value,
    pub output_redacted: Option<Value>,
    pub errors: Option<Value>,
    pub budget_snapshot: Option<Value>,
    pub timings: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct RunCall {
    pub seq: i32,
    pub api_call_slug: Slug,
    pub service_slug: Slug,
    pub method: http::Method,
    /// Already redacted at write time (`http::redact::redact_url`, via
    /// `runtime::recorder::row_from_pages`) — safe to expose verbatim (I4).
    pub url_redacted: String,
    /// Already redacted at write time (`http::redact::redact_headers`) — safe to expose verbatim.
    pub headers_redacted: Option<Value>,
    pub status_code: Option<u16>,
    pub response_bytes: Option<u64>,
    pub response_truncated: bool,
    pub error: Option<String>,
    /// The upstream's own response body for this call's last page, as persisted by
    /// `runtime::recorder::new_run_call` — the raw counterpart to the run's own
    /// `output_redacted` (which is the *projected* value). Exposed here because
    /// `server::api`'s test-run routes are the reason "raw vs. projected, side by side" needs
    /// to be recoverable after the fact, not just during the one dispatch that produced it.
    pub response_body: Option<Value>,
}

/// Narrows a [`RunSummary`] listing. `None` on any field means "no opinion" (no filter on that
/// axis); `limit`/`offset` always apply.
#[derive(Debug, Clone)]
pub struct RunFilter {
    pub endpoint_slug: Option<Slug>,
    pub status: Option<RunStatus>,
    pub limit: u64,
    pub offset: u64,
}

pub(super) fn target_kind_to_str(kind: RunTargetKind) -> &'static str {
    match kind {
        RunTargetKind::ApiCall => "api_call",
        RunTargetKind::Script => "script",
    }
}

pub(super) fn caller_kind_to_str(kind: RunCallerKind) -> &'static str {
    match kind {
        RunCallerKind::Oauth => "oauth",
        RunCallerKind::ServiceToken => "service_token",
    }
}

pub(super) fn status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Ok => "ok",
        RunStatus::Partial => "partial",
        RunStatus::Error => "error",
        RunStatus::Denied => "denied",
        RunStatus::BudgetExceeded => "budget_exceeded",
        RunStatus::Timeout => "timeout",
    }
}

pub(super) fn str_to_target_kind(s: &str) -> Result<RunTargetKind, StoreError> {
    match s {
        "api_call" => Ok(RunTargetKind::ApiCall),
        "script" => Ok(RunTargetKind::Script),
        other => Err(StoreError::Malformed(format!(
            "runs.target_kind: unrecognised value {other:?}"
        ))),
    }
}

pub(super) fn str_to_caller_kind(s: &str) -> Result<RunCallerKind, StoreError> {
    match s {
        "oauth" => Ok(RunCallerKind::Oauth),
        "service_token" => Ok(RunCallerKind::ServiceToken),
        other => Err(StoreError::Malformed(format!(
            "runs.caller_kind: unrecognised value {other:?}"
        ))),
    }
}

pub(super) fn str_to_status(s: &str) -> Result<RunStatus, StoreError> {
    match s {
        "ok" => Ok(RunStatus::Ok),
        "partial" => Ok(RunStatus::Partial),
        "error" => Ok(RunStatus::Error),
        "denied" => Ok(RunStatus::Denied),
        "budget_exceeded" => Ok(RunStatus::BudgetExceeded),
        "timeout" => Ok(RunStatus::Timeout),
        other => Err(StoreError::Malformed(format!(
            "runs.status: unrecognised value {other:?}"
        ))),
    }
}
