//! `runs`/`run_calls` façade — the audit trail. Not a "definition": writing a run never
//! bumps `meta.definitions_generation`. There is no `model::Run` (the audit trail isn't one
//! of the tool-definition aggregates `model` describes), so this store defines its own
//! plain, scalar-only request/response types, same reasoning as `store::user`.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Select, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{run_calls, runs};
use crate::model::Slug;

use super::run_types::{
    caller_kind_to_str, status_to_str, str_to_caller_kind, str_to_status, str_to_target_kind,
    target_kind_to_str,
};
use super::{StoreError, db_err, parse_slug};

pub use super::run_types::{
    NewRun, NewRunCall, RunCall, RunCallerKind, RunDetail, RunFilter, RunStatus, RunSummary,
    RunTargetKind,
};

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
            owner_id: Set(run.owner_id),
            endpoint_slug: Set(run.endpoint_slug.as_str().to_owned()),
            tool_name: Set(run.tool_name.clone()),
            target_kind: Set(target_kind_to_str(run.target_kind).to_owned()),
            target_slug: Set(run.target_slug.as_str().to_owned()),
            caller_kind: Set(caller_kind_to_str(run.caller_kind).to_owned()),
            caller_id: Set(run.caller_id.clone()),
            request_id: Set(run.request_id.clone()),
            execution_start: Set(run.execution_start.into()),
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

    /// `server::api`'s `GET /api/runs/{id}` — the one route allowed to carry `definition_snapshot`
    /// and every other column [`RunSummary`] leaves off (see [`RunDetail`]'s own doc). Scoped to
    /// `owner_id`: a run belonging to another owner comes back `None`, indistinguishable from a
    /// nonexistent id.
    pub async fn get(
        &self,
        owner_id: Uuid,
        id: Uuid,
    ) -> Result<Option<(RunDetail, Vec<RunCall>)>, StoreError> {
        let Some(row) = runs::Entity::find_by_id(id)
            .filter(runs::Column::OwnerId.eq(owner_id))
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
        Ok(Some((detail_to_model(row)?, calls)))
    }

    pub async fn list_for_endpoint(
        &self,
        owner_id: Uuid,
        endpoint_slug: &Slug,
        limit: u64,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let rows = runs::Entity::find()
            .filter(runs::Column::OwnerId.eq(owner_id))
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
    pub async fn list(
        &self,
        owner_id: Uuid,
        filter: &RunFilter,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let mut query: Select<runs::Entity> =
            runs::Entity::find().filter(runs::Column::OwnerId.eq(owner_id));
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

/// Builds the detail-route model. Clones `row` into [`summary_to_model`] rather than splitting
/// its fields by hand — `runs::Model` is a plain data row (`Clone`-derived, no I/O), so the clone
/// costs nothing worth avoiding, and this keeps the summary's own field mapping defined in
/// exactly one place.
fn detail_to_model(row: runs::Model) -> Result<RunDetail, StoreError> {
    let caller_kind = str_to_caller_kind(&row.caller_kind)?;
    let caller_id = row.caller_id.clone();
    let request_id = row.request_id.clone();
    let execution_start = row.execution_start.with_timezone(&Utc);
    let definition_snapshot = row.definition_snapshot.clone();
    let definition_digest = row.definition_digest.clone();
    let input_redacted = row.input_redacted.clone();
    let output_redacted = row.output_redacted.clone();
    let errors = row.errors.clone();
    let budget_snapshot = row.budget_snapshot.clone();
    let timings = row.timings.clone();
    Ok(RunDetail {
        summary: summary_to_model(row)?,
        caller_kind,
        caller_id,
        request_id,
        execution_start,
        definition_snapshot,
        definition_digest,
        input_redacted,
        output_redacted,
        errors,
        budget_snapshot,
        timings,
    })
}

fn call_to_model(row: run_calls::Model) -> Result<RunCall, StoreError> {
    let method = row.method.parse().map_err(|_| {
        StoreError::Malformed(format!(
            "run_calls.method: {:?} is not a valid HTTP method",
            row.method
        ))
    })?;
    Ok(RunCall {
        seq: row.seq,
        api_call_slug: parse_slug(&row.api_call_slug)?,
        service_slug: parse_slug(&row.service_slug)?,
        method,
        url_redacted: row.url_redacted,
        headers_redacted: row.headers_redacted,
        status_code: row.status_code.map(|v| v as u16),
        response_bytes: row.response_bytes.map(|v| v as u64),
        response_truncated: row.response_truncated,
        error: row.error,
        response_body: row.body_redacted,
    })
}
