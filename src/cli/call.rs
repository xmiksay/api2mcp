//! `api2mcp call <api_call_slug> --arg k=v ... [--raw] [--endpoint <slug>]` — the fastest
//! debugging loop the crate has: resolve the endpoint's plan, dispatch exactly once, print the
//! result.
//!
//! [`execute`] does **not** call [`crate::runtime::Executor::run_tool`] — its `RunResult` is
//! deliberately narrow (`run_id`/`status`/`value`/`error`, see that type's own module docs) and
//! never exposes the pre-projection [`crate::runtime::dispatch::DispatchOutcome`], so `--raw`
//! would have nothing to show. Instead it assembles the same public building blocks `run_tool`
//! itself uses for an api_call target — [`AuthProviders`], [`ConcurrencyLimits`],
//! [`BudgetMeter`], [`partial::run_batch`], [`recorder::record`] — which is exactly one HTTP
//! dispatch and one recorded `runs`/`run_calls` row, identical in shape to what `run_tool` would
//! have produced. `runtime::dispatch`'s own module doc names "the CLI" as one of the direct
//! callers its `pub` surface is built for, alongside a script's own `api()`/`api_many()`
//! bindings; this module is that caller.
//!
//! [`execute`] is split out from [`run`] (the CLI entry point) so `tests/cli.rs` can drive it
//! directly against a hand-built plan/pool, exactly like every other integration test in this
//! suite — never through `Config::from_env()`, which would mean fighting over process-global
//! environment variables between concurrently running tests (see `config`'s own module doc on
//! why `from_env`/`from_lookup` are split for the same reason).

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use uuid::Uuid;

use crate::config::Config;
use crate::db;
use crate::http::{CallResponse, HickoryDns, SsrfPolicy, UpstreamPool};
use crate::model::Slug;
use crate::resolve::plan::ToolTarget;
use crate::resolve::{self, EndpointPlan};
use crate::runtime::budget::BudgetMeter;
use crate::runtime::dispatch::{AuthProviders, DispatchContext};
use crate::runtime::fanout::ConcurrencyLimits;
use crate::runtime::partial::{self, BatchEntry, BatchStatus, ItemOutcome};
use crate::runtime::recorder::{self, RunRecord};
use crate::store::{RunCallerKind, RunStatus, Stores};

/// Every upstream request this CLI makes crosses `dispatch::dispatch` with this fixed redirect
/// cap — `Executor::new`'s own default, duplicated here since `Executor`'s fields are private to
/// `runtime`.
pub(crate) const MAX_REDIRECTS: u8 = 5;

/// The identity a CLI-initiated run is recorded under. `RunCallerKind` has exactly two variants
/// today, both meant for a network caller (see `server::mcp::handlers`'s own comment on why only
/// `ServiceToken` is reachable there); it's the closer fit of the two for a local, trusted,
/// human-operated process — `Oauth` implies a browser session that never existed here.
pub(crate) const CALLER_ID: &str = "cli";

/// One dispatched api_call, ready for a CLI to print: the recorded run's id and status, the
/// upstream response before projection, the tool's own projected result, and — whenever `status`
/// isn't `Ok` — a human-readable reason (a bad argument, an upstream failure, or a named budget
/// axis; see `entry_error_string`).
#[derive(Debug)]
pub struct CallOutcome {
    pub run_id: Uuid,
    pub status: RunStatus,
    pub raw: Option<Value>,
    pub projected: Option<Value>,
    pub error: Option<String>,
}

/// Connects, resolves `endpoint` (or `cfg.default_endpoint`) to a validated plan, and builds the
/// upstream pool the dispatch shares. Shared with `cli::script::run`. Reads `Config::from_env`,
/// so — like `cli::serve`/`cli::pack`/`cli::token` — this half is exercised by hand, not by
/// `tests/cli.rs`; see [`execute`]'s own doc for what *is* covered there.
pub(crate) async fn setup(
    endpoint: Option<&str>,
    user: Option<&str>,
) -> Result<(Stores, Arc<UpstreamPool>, SsrfPolicy, EndpointPlan)> {
    let cfg = Config::from_env()?;
    let conn = db::connect(&cfg.database_url).await?;
    let stores = Stores::new(conn);

    // No session or bearer token to derive an owner from here — see `cli::resolve_user`'s own
    // doc for why `--user` (defaulting to the sole user account) is the CLI's answer to the
    // question every other transport gets for free from `Caller`.
    let owner = crate::cli::resolve_user(&stores, user).await?;

    let slug_str = endpoint.unwrap_or(&cfg.default_endpoint);
    let endpoint_slug: Slug = slug_str
        .parse()
        .with_context(|| format!("{slug_str:?} is not a valid endpoint slug"))?;

    let policy = SsrfPolicy {
        allow_loopback: cfg.allow_loopback_upstream,
    };
    let pool = Arc::new(UpstreamPool::new(Arc::new(HickoryDns::new()), policy));

    let plan = resolve::build_plan(&stores, owner.id, &endpoint_slug)
        .await
        .with_context(|| format!("resolving endpoint {endpoint_slug:?}"))?;

    Ok((stores, pool, policy, plan))
}

/// Dispatches `name` once against `plan` and records the run. See the module doc for why this —
/// not `Executor::run_tool` — is what both `run` and `tests/cli.rs` call.
pub async fn execute(
    stores: &Stores,
    pool: &UpstreamPool,
    policy: SsrfPolicy,
    plan: &EndpointPlan,
    name: &str,
    args: Value,
) -> Result<CallOutcome> {
    let tool = plan
        .tool(name)
        .ok_or_else(|| anyhow!("no such tool {name:?} on endpoint {:?}", plan.slug.as_str()))?;

    // Shell input is text; `parse_args` had to guess a JSON type before the parameter list was
    // known. Now that it is, narrow that guess back where it over-reached.
    let args = match (&args, &tool.target) {
        (Value::Object(map), ToolTarget::ApiCall(slug)) => match plan.calls.get(slug) {
            Some(planned) => {
                let mut map = map.clone();
                crate::cli::retype_args_for_params(&planned.api_call.params, &mut map);
                Value::Object(map)
            }
            None => args,
        },
        _ => args,
    };

    let auth = AuthProviders::load(&stores.auth_provider(), plan)
        .await
        .context("loading auth providers")?;
    let ctx = DispatchContext {
        pool,
        policy: &policy,
        auth: &auth,
        max_redirects: MAX_REDIRECTS,
    };
    let limits = ConcurrencyLimits::build(plan, tool.budgets.max_concurrency);
    let meter = BudgetMeter::new(tool.budgets);

    let batch = partial::run_batch(
        plan,
        &ctx,
        &limits,
        &meter,
        None,
        vec![(name.to_owned(), args.clone())],
    )
    .await;

    // `run_batch` always returns exactly `items.len()` entries — one, here — so this can only be
    // `None` if that invariant breaks; errors rather than indexing/unwrapping into a panic.
    let entry = batch
        .entries
        .first()
        .ok_or_else(|| anyhow!("internal: run_batch returned no entries for a one-item batch"))?;
    let outcome = entry.outcome.dispatch_outcome();
    let projected = outcome.map(|o| o.value.clone());
    let raw = outcome.map(|o| raw_from_pages(&o.pages));
    let status = batch_status_to_run_status(batch.status);
    let error = (status != RunStatus::Ok).then(|| entry_error_string(entry));

    let request_id = Uuid::new_v4().to_string();
    let run_id = recorder::record(
        stores,
        RunRecord {
            plan,
            tool,
            // A CLI invocation is never a token, session or OAuth caller — it's the trusted
            // local operator running the binary directly (`server::identity::CallerKind::Cli`'s
            // own doc).
            caller_kind: RunCallerKind::Cli,
            caller_id: CALLER_ID.to_owned(),
            request_id,
            args,
            output_redacted: projected.clone(),
            status,
            entries: &batch.entries,
            meter: &meter,
            elapsed: None,
        },
    )
    .await
    .context("recording the run")?;

    Ok(CallOutcome {
        run_id,
        status,
        raw,
        projected,
        error,
    })
}

pub async fn run(
    name: &str,
    arg_pairs: &[String],
    raw: bool,
    endpoint: Option<&str>,
    user: Option<&str>,
) -> Result<()> {
    let args = Value::Object(crate::cli::parse_args(arg_pairs)?);
    let (stores, pool, policy, plan) = setup(endpoint, user).await?;
    let outcome = execute(&stores, &pool, policy, &plan, name, args).await?;

    println!("run_id: {}", outcome.run_id);
    println!("status: {}", status_label(outcome.status));
    if let Some(err) = &outcome.error {
        println!("error: {err}");
    }

    if raw {
        println!("\n--- raw upstream response ---");
        println!("{}", render(outcome.raw.as_ref()));
        println!("\n--- projected result ---");
        println!("{}", render(outcome.projected.as_ref()));
    } else {
        println!("\n{}", render(outcome.projected.as_ref()));
    }

    if outcome.status != RunStatus::Ok {
        bail!(
            "run {} did not succeed: {}",
            outcome.run_id,
            outcome.error.unwrap_or_default()
        );
    }
    Ok(())
}

/// Mirrors `runtime::to_run_status` (private to that module, hence duplicated) — see that
/// function's own docs for the judgment call this taxonomy encodes.
fn batch_status_to_run_status(status: BatchStatus) -> RunStatus {
    match status {
        BatchStatus::Ok => RunStatus::Ok,
        BatchStatus::Partial => RunStatus::Partial,
        BatchStatus::AllFailedOnBudget(crate::runtime::budget::BudgetAxis::WallClock) => {
            RunStatus::Timeout
        }
        BatchStatus::AllFailedOnBudget(_) => RunStatus::BudgetExceeded,
        BatchStatus::AllFailed => RunStatus::Error,
    }
}

pub(crate) fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Ok => "ok",
        RunStatus::Partial => "partial",
        RunStatus::Error => "error",
        RunStatus::Denied => "denied",
        RunStatus::BudgetExceeded => "budget_exceeded",
        RunStatus::Timeout => "timeout",
    }
}

fn entry_error_string(entry: &BatchEntry) -> String {
    match &entry.outcome {
        ItemOutcome::Ok(_) => String::new(),
        ItemOutcome::Failed(e) => e.to_string(),
        ItemOutcome::NotAttempted(axis) => format!("budget exceeded: {}", axis.as_str()),
        ItemOutcome::BudgetCut(axis, _) => format!("budget exceeded: {}", axis.as_str()),
    }
}

/// One line per script call for `script run`'s breakdown — which api_call ran, in input order,
/// its outcome, and how many bytes it moved. Shared with `cli::script`.
pub(crate) fn format_call_entry(entry: &BatchEntry) -> String {
    match &entry.outcome {
        ItemOutcome::Ok(o) => format!(
            "[{}] {} ({}){} -> ok, {} bytes",
            entry.index,
            entry.name,
            o.api_call_slug,
            http_status_suffix(o),
            o.bytes_in()
        ),
        ItemOutcome::Failed(e) => format!("[{}] {} -> failed: {e}", entry.index, entry.name),
        ItemOutcome::NotAttempted(axis) => format!(
            "[{}] {} -> not attempted (budget exceeded: {})",
            entry.index,
            entry.name,
            axis.as_str()
        ),
        ItemOutcome::BudgetCut(axis, o) => format!(
            "[{}] {} ({}){} -> budget exceeded ({}) after {} bytes",
            entry.index,
            entry.name,
            o.api_call_slug,
            http_status_suffix(o),
            axis.as_str(),
            o.bytes_in()
        ),
    }
}

fn http_status_suffix(outcome: &crate::runtime::dispatch::DispatchOutcome) -> String {
    outcome
        .pages
        .last()
        .map(|p| format!(" [{}]", p.status.as_u16()))
        .unwrap_or_default()
}

/// Parses each fetched page's body as JSON and collapses to a single value (or an array across
/// pages) exactly like `runtime::dispatch`'s own (private) `project_pages`, minus the projection
/// step — so `--raw` and the projected result differ by exactly that one step, which is the
/// whole point of printing them side by side.
fn raw_from_pages(pages: &[CallResponse]) -> Value {
    let mut values: Vec<Value> = pages
        .iter()
        .map(|p| {
            if p.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&p.body).unwrap_or_else(|_| {
                    Value::String(String::from_utf8_lossy(&p.body).into_owned())
                })
            }
        })
        .collect();
    match values.len() {
        1 => values.remove(0),
        _ => Value::Array(values),
    }
}

pub(crate) fn render(value: Option<&Value>) -> String {
    match value {
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()),
        None => "(no value)".to_owned(),
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
