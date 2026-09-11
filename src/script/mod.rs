//! The Rhai scripting layer (Fáze 3) — I1's script-facing half. A script never performs HTTP; its
//! only capabilities are `api`/`api_many`/`api_try` ([`bindings`]), each of which crosses to the
//! async side over a channel ([`bridge`]) and resolves through [`crate::runtime::dispatch`] — the
//! one function in the crate that can reach `http::send`. See `bindings`/`bridge`'s own module
//! docs for exactly how I1 is enforced and why it lives there rather than in registration.
//!
//! [`run_script`] is this chunk's entry point: bind the script's own declared params into scope,
//! run the engine under `spawn_blocking`, service its binding calls on this task until it
//! finishes, then join. A panic in the blocking closure becomes a [`ScriptFailure`] rather than
//! taking the process down — `spawn_blocking`'s `JoinHandle` already turns a panic into an `Err`,
//! so this is a `match`, not a `catch_unwind`.
//!
//! **Budgets terminate, they do not throw.** [`engine::build_engine`]'s `on_progress` hook trips
//! once the run's own wall-clock deadline passes, producing `EvalAltResult::ErrorTerminated` — the
//! one error class a script's `try`/`catch` cannot swallow. The second path matters equally: while
//! the script is parked in `blocking_recv` waiting on a batch, `on_progress` cannot run at all, so
//! [`bridge::service`] independently checks the same deadline (via `run_batch`'s own
//! `remaining_time()`) and replies with [`bridge::BridgeReply::Terminated`] instead of an ordinary
//! per-item result — see `bridge`'s module docs. Both paths converge on the same uncatchable
//! error.
//!
//! **Not this chunk's job** (see `runtime::mod`'s own docs on what it deliberately doesn't do
//! yet): wiring `run_script` into `Executor::run_tool`, and a resolve-level cache of compiled
//! `AST`s across repeated runs of the same [`ScriptDef`]. Both are later-chunk concerns; this
//! module is exercised directly by its own tests in the meantime.

pub mod bindings;
pub mod bridge;
pub mod engine;
pub mod errors;
pub mod marshal;

use std::sync::Mutex;
use std::time::Instant;

use rhai::{Dynamic, Scope};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::model::{ScriptDef, Slug};
use crate::resolve::EndpointPlan;
use crate::runtime::budget::BudgetMeter;
use crate::runtime::dispatch::DispatchContext;
use crate::runtime::fanout::ConcurrencyLimits;
use crate::runtime::partial::BatchEntry;
use crate::schema;

pub use errors::{RunScriptError, ScriptFailure, ScriptFailureKind};

/// Runs `script` against `args`, sharing `plan`/`ctx`/`limits`/`meter` with whatever else the
/// caller's run does — the same four values `Executor::run_tool` builds once per run, not once
/// per script call (`runtime::mod`'s own module docs on what this chunk still has to wire up).
///
/// Argument binding happens first, synchronously, against the script's own declared params
/// ([`ScriptDef::params`]) — a caller-side mismatch is [`RunScriptError::Args`], distinct from
/// the script itself misbehaving ([`RunScriptError::Script`]).
/// A finished script run: what it returned, plus every upstream call it made along the way so
/// the recorder can write a `run_calls` row for each. A script's calls are made inside the
/// bridge rather than by the caller, so without collecting them here they would be invisible to
/// the audit trail.
#[derive(Debug)]
pub struct ScriptRun {
    pub value: Value,
    pub calls: Vec<BatchEntry>,
}

pub async fn run_script(
    plan: &EndpointPlan,
    ctx: &DispatchContext<'_>,
    limits: &ConcurrencyLimits,
    meter: &BudgetMeter,
    caller_script: &Slug,
    script: &ScriptDef,
    args: Value,
) -> Result<ScriptRun, RunScriptError> {
    let bound = schema::bind_args(&script.params, &args).map_err(RunScriptError::Args)?;
    let scope_vars: Vec<(String, Value)> = bound.into_iter().collect();

    let (tx, rx) = mpsc::unbounded_channel::<bridge::BindingCall>();
    let deadline = wall_clock_deadline(meter);
    let source = script.source.clone();

    let handle =
        tokio::task::spawn_blocking(move || run_blocking(&source, scope_vars, deadline, tx));

    let audit = Mutex::new(Vec::new());
    bridge::service(rx, plan, ctx, limits, meter, caller_script, &audit).await;
    let calls = audit.into_inner().unwrap_or_default();

    match handle.await {
        Ok(Ok(value)) => Ok(ScriptRun { value, calls }),
        Ok(Err(failure)) => Err(RunScriptError::Script(failure)),
        // The blocking closure panicked instead of returning — never allowed to take the process
        // down with it.
        Err(join_err) => Err(RunScriptError::Script(ScriptFailure::panic(
            join_err.to_string(),
        ))),
    }
}

/// The whole synchronous half, run entirely inside `spawn_blocking`: build the engine, compile
/// once, bind the script's own params into scope, evaluate, marshal the return value. Nothing
/// here may touch the async runtime — every capability that needs one crosses via `tx`.
fn run_blocking(
    source: &str,
    scope_vars: Vec<(String, Value)>,
    deadline: Option<Instant>,
    tx: mpsc::UnboundedSender<bridge::BindingCall>,
) -> Result<Value, ScriptFailure> {
    let mut eng = engine::build_engine(deadline);
    bindings::register(&mut eng, tx);

    let ast = engine::compile(&eng, source)?;

    let mut scope = Scope::new();
    for (name, value) in &scope_vars {
        let dyn_value = marshal::to_dynamic(value)
            .map_err(|e| ScriptFailure::runtime(format!("binding parameter {name:?}: {e}")))?;
        scope.push_dynamic(name.clone(), dyn_value);
    }

    let result = eng
        .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
        .map_err(|e| ScriptFailure::from_eval_error(source, &e))?;

    marshal::to_json(&result)
        .map_err(|e| ScriptFailure::runtime(format!("script return value: {e}")))
}

/// A snapshot `Instant` deadline taken once, up front. [`BudgetMeter`] can't cross into
/// `spawn_blocking`'s `'static` closure by reference, so this is the one value pulled out of it
/// that can: the engine's `on_progress` hook only ever needs "has the run's wall clock passed",
/// never the meter's other axes (calls/bytes/pages stay entirely on the async side, in
/// [`bridge`], via `run_batch`/`dispatch`).
fn wall_clock_deadline(meter: &BudgetMeter) -> Option<Instant> {
    match meter.remaining_time() {
        Ok(Some(remaining)) => Some(Instant::now() + remaining),
        Ok(None) => None,
        // Already expired: an instant in the past trips `on_progress` on the very first
        // operation the engine performs.
        Err(_) => Some(Instant::now()),
    }
}
