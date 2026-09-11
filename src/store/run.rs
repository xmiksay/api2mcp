//! `runs`/`run_calls` façade — the audit trail. Not a "definition": writing a run never
//! bumps `meta.definitions_generation`. There is no `model::Run` (the audit trail isn't one
//! of the tool-definition aggregates `model` describes), so this store defines its own
//! plain, scalar-only request/response types, same reasoning as `store::user`.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Select, Set, TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use crate::entity::{run_calls, runs};
use crate::model::Slug;

use super::{StoreError, db_err, parse_slug};

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

#[derive(Debug, Clone)]
pub struct RunCall {
    pub seq: i32,
    pub api_call_slug: Slug,
    pub service_slug: Slug,
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

#[derive(Clone)]
pub struct RunStore {
    db: DatabaseConnection,
}

impl RunStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn create(&self, run: &NewRun, calls: &[NewRunCall]) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        let txn = self.db.begin().await.map_err(db_err("run::create"))?;
        runs::ActiveModel {
            id: Set(id),
            endpoint_slug: Set(run.endpoint_slug.as_str().to_owned()),
            tool_name: Set(run.tool_name.clone()),
            target_kind: Set(target_kind_to_str(run.target_kind).to_owned()),
            target_slug: Set(run.target_slug.as_str().to_owned()),
            caller_kind: Set(caller_kind_to_str(run.caller_kind).to_owned()),
            caller_id: Set(run.caller_id.clone()),
            request_id: Set(run.request_id.clone()),
            definition_snapshot: Set(run.definition_snapshot.clone()),
            definition_digest: Set(run.definition_digest.clone()),
            input_redacted: Set(run.input_redacted.clone()),
            output_redacted: Set(run.output_redacted.clone()),
            status: Set(status_to_str(run.status).to_owned()),
            errors: Set(run.errors.clone()),
            calls_made: Set(run.calls_made as i32),
            bytes_in: Set(run.bytes_in as i64),
            pages_fetched: Set(run.pages_fetched as i32),
            budget_snapshot: Set(run.budget_snapshot.clone()),
            timings: Set(run.timings.clone()),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(db_err("run::create"))?;

        for call in calls {
            run_calls::ActiveModel {
                id: Set(Uuid::new_v4()),
                run_id: Set(id),
                seq: Set(call.seq),
                api_call_slug: Set(call.api_call_slug.as_str().to_owned()),
                service_slug: Set(call.service_slug.as_str().to_owned()),
                method: Set(call.method.to_string()),
                url_redacted: Set(call.url_redacted.clone()),
                headers_redacted: Set(call.headers_redacted.clone()),
                body_redacted: Set(call.body_redacted.clone()),
                status_code: Set(call.status_code.map(|v| v as i32)),
                response_bytes: Set(call.response_bytes.map(|v| v as i64)),
                response_truncated: Set(call.response_truncated),
                error: Set(call.error.clone()),
                timings: Set(call.timings.clone()),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(db_err("run::create"))?;
        }

        txn.commit().await.map_err(db_err("run::create"))?;
        Ok(id)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<(RunSummary, Vec<RunCall>)>, StoreError> {
        let Some(row) = runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("run::get"))?
        else {
            return Ok(None);
        };
        let call_rows = run_calls::Entity::find()
            .filter(run_calls::Column::RunId.eq(id))
            .order_by_asc(run_calls::Column::Seq)
            .all(&self.db)
            .await
            .map_err(db_err("run::get"))?;
        let calls = call_rows
            .into_iter()
            .map(call_to_model)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((summary_to_model(row)?, calls)))
    }

    pub async fn list_for_endpoint(
        &self,
        endpoint_slug: &Slug,
        limit: u64,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let rows = runs::Entity::find()
            .filter(runs::Column::EndpointSlug.eq(endpoint_slug.as_str()))
            .order_by_desc(runs::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(db_err("run::list_for_endpoint"))?;
        rows.into_iter().map(summary_to_model).collect()
    }

    /// `server::api`'s `GET /api/runs`: an optional endpoint/status filter plus offset paging —
    /// [`Self::list_for_endpoint`] alone can't express either, and neither is a rule this crate's
    /// admin UI can do without once the run log has more than a page's worth of history.
    pub async fn list(&self, filter: &RunFilter) -> Result<Vec<RunSummary>, StoreError> {
        let mut query: Select<runs::Entity> = runs::Entity::find();
        if let Some(slug) = &filter.endpoint_slug {
            query = query.filter(runs::Column::EndpointSlug.eq(slug.as_str()));
        }
        if let Some(status) = filter.status {
            query = query.filter(runs::Column::Status.eq(status_to_str(status)));
        }
        let rows = query
            .order_by_desc(runs::Column::CreatedAt)
            .limit(filter.limit)
            .offset(filter.offset)
            .all(&self.db)
            .await
            .map_err(db_err("run::list"))?;
        rows.into_iter().map(summary_to_model).collect()
    }
}

fn target_kind_to_str(kind: RunTargetKind) -> &'static str {
    match kind {
        RunTargetKind::ApiCall => "api_call",
        RunTargetKind::Script => "script",
    }
}

fn caller_kind_to_str(kind: RunCallerKind) -> &'static str {
    match kind {
        RunCallerKind::Oauth => "oauth",
        RunCallerKind::ServiceToken => "service_token",
    }
}

fn status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Ok => "ok",
        RunStatus::Partial => "partial",
        RunStatus::Error => "error",
        RunStatus::Denied => "denied",
        RunStatus::BudgetExceeded => "budget_exceeded",
        RunStatus::Timeout => "timeout",
    }
}

fn str_to_target_kind(s: &str) -> Result<RunTargetKind, StoreError> {
    match s {
        "api_call" => Ok(RunTargetKind::ApiCall),
        "script" => Ok(RunTargetKind::Script),
        other => Err(StoreError::Malformed(format!(
            "runs.target_kind: unrecognised value {other:?}"
        ))),
    }
}

fn str_to_status(s: &str) -> Result<RunStatus, StoreError> {
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

fn summary_to_model(row: runs::Model) -> Result<RunSummary, StoreError> {
    Ok(RunSummary {
        id: row.id,
        endpoint_slug: parse_slug(&row.endpoint_slug)?,
        tool_name: row.tool_name,
        target_kind: str_to_target_kind(&row.target_kind)?,
        target_slug: parse_slug(&row.target_slug)?,
        status: str_to_status(&row.status)?,
        calls_made: row.calls_made as u32,
        bytes_in: row.bytes_in as u64,
        pages_fetched: row.pages_fetched as u32,
        created_at: row.created_at.with_timezone(&Utc),
    })
}

fn call_to_model(row: run_calls::Model) -> Result<RunCall, StoreError> {
    Ok(RunCall {
        seq: row.seq,
        api_call_slug: parse_slug(&row.api_call_slug)?,
        service_slug: parse_slug(&row.service_slug)?,
        status_code: row.status_code.map(|v| v as u16),
        response_bytes: row.response_bytes.map(|v| v as u64),
        response_truncated: row.response_truncated,
        error: row.error,
        response_body: row.body_redacted,
    })
}
