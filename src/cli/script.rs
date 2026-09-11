//! `api2mcp script run <script_slug> --arg k=v ... [--endpoint <slug>]` — the script-composing
//! sibling of `cli::call`. See that module's own doc for why [`execute`] doesn't call
//! [`crate::runtime::Executor::run_tool`] directly: `RunResult` never exposes the per-call
//! breakdown a composed script needs to be debuggable, but [`crate::script::ScriptRun::calls`]
//! does, so this assembles the same building blocks `run_tool`'s script branch uses
//! ([`AuthProviders`], [`ConcurrencyLimits`], [`BudgetMeter`], [`crate::script::run_script`],
//! [`recorder::record`]) directly — and, same reasoning as `cli::call`, is why `execute` (not
//! `run`) is what `tests/cli.rs` drives.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use uuid::Uuid;

use crate::http::{SsrfPolicy, UpstreamPool};
use crate::model::Slug;
use crate::resolve::EndpointPlan;
use crate::resolve::plan::ToolTarget;
use crate::runtime::budget::BudgetMeter;
use crate::runtime::dispatch::{AuthProviders, DispatchContext};
use crate::runtime::fanout::ConcurrencyLimits;
use crate::runtime::partial::BatchEntry;
use crate::runtime::recorder::{self, RunRecord};
use crate::script::{RunScriptError, ScriptFailure};
use crate::store::{RunCallerKind, RunStatus, Stores};

use super::call::{self, CALLER_ID, MAX_REDIRECTS};

/// One script run, ready for a CLI to print: the recorded run's id and status, the returned
/// value (on success), the per-call breakdown the script's own `api()`/`api_many()` calls made
/// (in input order — empty when the script never got far enough to make any, or failed before
/// any completed), and — on failure — the [`RunScriptError`] itself, so a caller can tell a bad
/// argument apart from a script that ran and threw.
#[derive(Debug)]
pub struct ScriptOutcome {
    pub run_id: Uuid,
    pub status: RunStatus,
    pub value: Option<Value>,
    pub calls: Vec<BatchEntry>,
    pub error: Option<RunScriptError>,
}

/// Runs `name` (a script tool) once against `plan` and records the run. See the module doc for
/// why this — not `Executor::run_tool` — is what both `run` and `tests/cli.rs` call.
pub async fn execute(
    stores: &Stores,
    pool: &UpstreamPool,
    policy: SsrfPolicy,
    plan: &EndpointPlan,
    name: &str,
    args: Value,
) -> Result<ScriptOutcome> {
    let tool = plan
        .tool(name)
        .ok_or_else(|| anyhow!("no such tool {name:?} on endpoint {:?}", plan.slug.as_str()))?;
    let script_slug: &Slug = match &tool.target {
        ToolTarget::Script(slug) => slug,
        ToolTarget::ApiCall(_) => {
            bail!("{name:?} is an api_call, not a script — use `api2mcp call` instead")
        }
    };
    // `tool.target` was resolved from `plan.tools`, which `resolve::build_plan` only ever
    // populates from `plan.scripts`' own keys — this can only be absent if that invariant
    // breaks, so this errors rather than indexing/unwrapping into a panic.
    let script = plan.scripts.get(script_slug).ok_or_else(|| {
        anyhow!("internal: {script_slug} resolved as a tool but is absent from plan.scripts")
    })?;

    // Shell input is text; `parse_args` had to guess a JSON type before the parameter list was
    // known. Now that it is, narrow that guess back where it over-reached.
    let args = match &args {
        Value::Object(map) => {
            let mut map = map.clone();
            crate::cli::retype_args_for_params(&script.params, &mut map);
            Value::Object(map)
        }
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

    let script_result = crate::script::run_script(
        plan,
        &ctx,
        &limits,
        &meter,
        script_slug,
        script,
        args.clone(),
    )
    .await;

    // A caller-argument or in-script failure never produced a `ScriptRun` — and therefore no
    // per-call breakdown to persist or print either (`script::run_script`'s own doc: the calls a
    // failed script made before failing aren't surfaced through its return type).
    let (status, value, calls, error) = match script_result {
        Ok(run) => (RunStatus::Ok, Some(run.value), run.calls, None),
        Err(e) => (RunStatus::Error, None, Vec::new(), Some(e)),
    };

    let request_id = Uuid::new_v4().to_string();
    let run_id = recorder::record(
        stores,
        RunRecord {
            plan,
            tool,
            caller_kind: RunCallerKind::ServiceToken,
            caller_id: CALLER_ID.to_owned(),
            request_id,
            args,
            output_redacted: value.clone(),
            status,
            entries: &calls,
            meter: &meter,
            elapsed: None,
        },
    )
    .await
    .context("recording the run")?;

    Ok(ScriptOutcome {
        run_id,
        status,
        value,
        calls,
        error,
    })
}

pub async fn run(name: &str, arg_pairs: &[String], endpoint: Option<&str>) -> Result<()> {
    let args = Value::Object(crate::cli::parse_args(arg_pairs)?);
    let (stores, pool, policy, plan) = call::setup(endpoint).await?;
    let outcome = execute(&stores, &pool, policy, &plan, name, args).await?;

    println!("run_id: {}", outcome.run_id);
    println!("status: {}", call::status_label(outcome.status));

    if !outcome.calls.is_empty() {
        println!("\ncalls (input order):");
        for entry in &outcome.calls {
            println!("  {}", call::format_call_entry(entry));
        }
    }

    match &outcome.error {
        None => {
            println!("\n{}", call::render(outcome.value.as_ref()));
            Ok(())
        }
        Some(RunScriptError::Args(e)) => {
            bail!("run {} did not succeed: bad arguments: {e}", outcome.run_id);
        }
        Some(RunScriptError::Script(failure)) => {
            print_script_failure(failure);
            bail!(
                "run {} did not succeed: script {:?}: {}",
                outcome.run_id,
                failure.kind,
                failure.message
            );
        }
    }
}

/// Prints a `ScriptFailure`'s line, column and caret-annotated source snippet — what makes a
/// script debuggable from the terminal instead of just "it failed".
fn print_script_failure(failure: &ScriptFailure) {
    eprintln!("\nscript failed ({:?}): {}", failure.kind, failure.message);
    match (failure.line, failure.column) {
        (Some(line), Some(column)) => eprintln!("  at line {line}, column {column}"),
        (Some(line), None) => eprintln!("  at line {line}"),
        (None, _) => {}
    }
    if let Some(snippet) = &failure.snippet {
        eprintln!("{snippet}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::ScriptFailureKind;

    #[test]
    fn print_script_failure_does_not_panic_with_no_position_info() {
        // Exercises the (None, _) arm; the real assertion is "this returns" (stderr isn't
        // capturable here without adding a stdout/stderr redirection dependency for one test).
        print_script_failure(&ScriptFailure {
            kind: ScriptFailureKind::Runtime,
            message: "boom".into(),
            line: None,
            column: None,
            snippet: None,
        });
    }
}
