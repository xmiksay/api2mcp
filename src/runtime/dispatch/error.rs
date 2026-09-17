//! [`DispatchError`] and the [`DispatchAttempt`] it can carry — split out of `dispatch/mod.rs`
//! purely to keep that file under the workspace's 400-line cap (same reasoning as
//! `http/bind.rs`'s `bind_tests.rs` split), not because either type belongs to a different
//! concern: `dispatch::dispatch` is still both types' only producer.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::http::{CallError, CallResponse, PaginateError};
use crate::model::Slug;
use crate::project::ProjectionError;
use crate::schema::ValidationError;

use super::super::budget::BudgetAxis;

/// The request that actually reached the wire, carried on a [`DispatchError`] for the three
/// failure kinds that can only happen **after** [`crate::http::paginate`] already returned real
/// upstream pages (a non-2xx status, a body that isn't valid JSON, or a projection failure) —
/// never for a failure that pre-dates sending (bad args, a bind failure, a client-build failure)
/// or one where `paginate` itself never produced a response (a guard rejection, a timeout, a
/// transport error). This lives on the error rather than as a new [`super::super::partial::ItemOutcome`]
/// variant on purpose: `ItemOutcome::Failed` stays a plain `DispatchError` newtype, so every
/// existing exhaustive match on it (the script bridge's error-object builder among them) keeps
/// compiling unchanged, and `runtime::recorder` is the only place that ever reads this field.
#[derive(Debug, Clone)]
pub struct DispatchAttempt {
    pub api_call_slug: Slug,
    pub service_slug: Slug,
    pub method: ::http::Method,
    pub request_headers: BTreeMap<String, String>,
    pub request_body: Option<Value>,
    pub pages: Vec<CallResponse>,
}

/// Errors from `dispatch::dispatch`. `thiserror` + `Serialize` — never `anyhow` — because a
/// per-item dispatch failure can land directly in a run's `errors[]` column.
/// The `kind` tag is a **stable contract**: a script branches on it (`if e.kind == "http_status"`)
/// and a run's persisted `errors[]` records it, so renaming a variant is a breaking change to
/// both surfaces at once.
#[derive(Debug, Clone, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchError {
    #[error("{name:?} is not declared for this caller")]
    NotDeclared { name: String },
    #[error("{name:?} is a script, not an api_call — it cannot be dispatched directly")]
    NotAnApiCall { name: String },
    #[error("arguments: {0}")]
    Args(#[from] ValidationError),
    #[error("binding request: {0}")]
    Bind(#[from] crate::http::BindError),
    #[error("sending request: {0}")]
    Send(#[from] PaginateError),
    #[error("building the upstream client: {0}")]
    Client(String),
    #[error("upstream returned {status} {reason}: {detail}")]
    HttpStatus {
        status: u16,
        reason: String,
        /// A short, redacted excerpt of the upstream's own error body. Upstreams explain a 422
        /// far better than we can, and a model that can read that explanation can often fix its
        /// own arguments and retry.
        detail: String,
        /// See [`DispatchAttempt`]'s own doc. Never serialized: this error crosses into a
        /// script-visible JSON value and the run's own persisted `errors[]` (via `Display`/
        /// `to_string`, never this `Serialize` impl, but the field still must not be
        /// serializable — see `runtime::recorder`'s module doc on why a response body must never
        /// ride inside it), and it can carry raw, unredacted upstream bytes.
        #[serde(skip)]
        attempt: Option<Box<DispatchAttempt>>,
    },
    #[error("response was not valid JSON: {message}")]
    ResponseNotJson {
        message: String,
        #[serde(skip)]
        attempt: Option<Box<DispatchAttempt>>,
    },
    #[error("projection: {source}")]
    Projection {
        #[source]
        source: ProjectionError,
        #[serde(skip)]
        attempt: Option<Box<DispatchAttempt>>,
    },
}

impl DispatchError {
    /// Whether this error is a budget trip wearing a per-item error's clothes — see the module
    /// docs on `runtime::budget` for why a wall-clock timeout and a page-cap hit collapse onto
    /// [`BudgetAxis`] rather than surfacing as an ordinary per-item failure a script's
    /// `try`/`catch` could swallow.
    ///
    /// **One subtlety this deliberately flattens.** `BudgetAxis::WallClock` here is *this call's
    /// own* `timeout_ms` elapsing, which is not the same event as the run's total wall-clock
    /// budget being exhausted — they share a variant only because the question being asked is
    /// "are this batch's failures uniformly time-related", and for that they are equivalent.
    ///
    /// The consequence is worth knowing before changing anything here, because it is what a
    /// script author actually observes: a single call timing out stays an ordinary catchable
    /// `send` error, and it is only when *every* item in the same `api_many` batch trips the same
    /// axis that `runtime::partial` escalates the whole call to an uncatchable termination. So
    /// one slow upstream among ten is a per-item failure a script can handle, while ten slow
    /// upstreams end the run. Tracing that took three files to confirm; it is written down here
    /// so the next person does not have to.
    pub fn budget_trip(&self) -> Option<BudgetAxis> {
        match self {
            DispatchError::Send(PaginateError::PageCapExceeded { .. }) => Some(BudgetAxis::Pages),
            DispatchError::Send(PaginateError::Call(CallError::Timeout)) => {
                Some(BudgetAxis::WallClock)
            }
            _ => None,
        }
    }

    /// The attempt that produced this failure, if any — see [`DispatchAttempt`]'s own doc for
    /// exactly which variants ever carry one. `runtime::recorder` is this method's only caller:
    /// it is what turns "the request genuinely went out" into a `run_calls` row even though the
    /// call as a whole failed.
    pub fn attempt(&self) -> Option<&DispatchAttempt> {
        match self {
            DispatchError::HttpStatus { attempt, .. }
            | DispatchError::ResponseNotJson { attempt, .. }
            | DispatchError::Projection { attempt, .. } => attempt.as_deref(),
            _ => None,
        }
    }

    /// Attaches `attempt` to whichever of the three post-`paginate` variants `self` is; a no-op
    /// on every other variant, since those never reached the wire in the first place.
    pub(super) fn with_attempt(mut self, attempt: DispatchAttempt) -> Self {
        let slot = match &mut self {
            DispatchError::HttpStatus { attempt, .. }
            | DispatchError::ResponseNotJson { attempt, .. }
            | DispatchError::Projection { attempt, .. } => attempt,
            _ => return self,
        };
        *slot = Some(Box::new(attempt));
        self
    }
}
