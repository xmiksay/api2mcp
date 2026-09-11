//! `api`, `api_many`, `api_try` — the only three script-callable functions that can ever reach
//! HTTP, and even they don't directly: each marshals its arguments, sends a [`BindingCall`]
//! across the bridge, and blocks for a JSON reply. I1's actual enforcement lives on the async
//! side ([`super::bridge`], via `runtime::dispatch::resolve`) — see that module's docs for why
//! that's the one place to audit rather than here.
//!
//! **Never register a binding named `call`.** It is the hard-reserved `KEYWORD_FN_PTR_CALL`
//! (`rhai::engine::KEYWORD_FN_PTR_CALL == "call"`); the interpreter special-cases it in
//! `make_function_call` to always mean "invoke this `FnPtr`", so `register_fn("call", ...)` is
//! *silently shadowed* rather than failing loudly — its first argument gets coerced as a function
//! pointer instead of ever reaching our closure. `entanglement` hit exactly this and renamed its
//! own binding to `exec`. The same interpreter special-casing rules out `fn` (a keyword, not a
//! constant, but never a legal binding name either), `this` (`KEYWORD_THIS`), `type_of`
//! (`KEYWORD_TYPE_OF`), `is_def_fn` (`KEYWORD_IS_DEF_FN`), `is_def_var` (`KEYWORD_IS_DEF_VAR`),
//! `Fn` (`KEYWORD_FN_PTR`), and `curry` (`KEYWORD_FN_PTR_CURRY`).

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, NativeCallContext};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{BindingCall, BridgeReply};
use super::marshal;

/// Names rhai's interpreter special-cases and would silently misinterpret a same-named
/// `register_fn` for (see the module docs). Not something there is a runtime check to perform —
/// this is the documentation [`BINDING_NAMES`] must keep disjoint from, pinned by a unit test.
pub const RESERVED_BY_RHAI: &[&str] = &[
    "call",
    "fn",
    "this",
    "type_of",
    "is_def_fn",
    "is_def_var",
    "Fn",
    "curry",
];

/// The three script-facing binding names this module registers.
pub const BINDING_NAMES: &[&str] = &["api", "api_many", "api_try"];

/// Registers `api`/`api_many`/`api_try` on `engine`, each holding its own clone of `tx` so the
/// bridge channel only closes once every one of them (and the engine that owns them) is dropped.
pub fn register(engine: &mut Engine, tx: mpsc::UnboundedSender<BindingCall>) {
    let t = tx.clone();
    engine.register_fn(
        "api",
        move |ctx: NativeCallContext, name: &str, args: Map| api_call(&t, ctx, name, args),
    );

    let t = tx.clone();
    engine.register_fn(
        "api_try",
        move |ctx: NativeCallContext, name: &str, args: Map| api_try_call(&t, ctx, name, args),
    );

    engine.register_fn(
        "api_many",
        move |ctx: NativeCallContext, name: &str, args_list: Array| {
            api_many_call(&tx, ctx, name, args_list)
        },
    );
}

/// Sends one batch across the bridge and blocks for the reply. `blocking_recv` is safe here
/// because this closure only ever runs on the `spawn_blocking` thread the engine itself runs on
/// — a thread with no entered Tokio runtime guard, per this module's own docs.
fn send_batch(
    tx: &mpsc::UnboundedSender<BindingCall>,
    ctx: &NativeCallContext,
    name: &str,
    batch: Vec<Value>,
) -> Result<Vec<Value>, Box<EvalAltResult>> {
    let (reply, wait) = oneshot::channel();
    tx.send(BindingCall {
        name: name.to_owned(),
        batch,
        reply,
    })
    .map_err(|_| runtime_err(ctx, "script host bridge closed"))?;

    match wait.blocking_recv() {
        Ok(BridgeReply::Entries(entries)) => Ok(entries),
        Ok(BridgeReply::Refused(msg)) => Err(runtime_err(ctx, &msg)),
        // The one class of error a script's own `try`/`catch` cannot swallow — see
        // `super::bridge`'s docs for the two paths that produce this.
        Ok(BridgeReply::Terminated(msg)) => Err(Box::new(EvalAltResult::ErrorTerminated(
            Dynamic::from(msg),
            ctx.call_position(),
        ))),
        Err(_) => Err(runtime_err(ctx, "script host bridge dropped")),
    }
}

fn api_call(
    tx: &mpsc::UnboundedSender<BindingCall>,
    ctx: NativeCallContext,
    name: &str,
    args: Map,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let json_args = marshal::to_json(&Dynamic::from(args))
        .map_err(|e| runtime_err(&ctx, &format!("api: {e}")))?;
    let entries = send_batch(tx, &ctx, name, vec![json_args])?;
    let entry = first_entry(&ctx, entries, "api")?;
    entry_to_value(&ctx, entry)
}

fn api_try_call(
    tx: &mpsc::UnboundedSender<BindingCall>,
    ctx: NativeCallContext,
    name: &str,
    args: Map,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let json_args = marshal::to_json(&Dynamic::from(args))
        .map_err(|e| runtime_err(&ctx, &format!("api_try: {e}")))?;
    let entries = send_batch(tx, &ctx, name, vec![json_args])?;
    let entry = first_entry(&ctx, entries, "api_try")?;
    // Never throws for a per-item outcome — the whole point of `api_try`: the result shape
    // matches an `api_many` element (`ok`/`index`/`value`|`error`) so a script can use one error
    // idiom throughout, regardless of which of the three functions it called.
    marshal::to_dynamic(&entry).map_err(|e| runtime_err(&ctx, &e.to_string()))
}

fn api_many_call(
    tx: &mpsc::UnboundedSender<BindingCall>,
    ctx: NativeCallContext,
    name: &str,
    args_list: Array,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let mut batch = Vec::with_capacity(args_list.len());
    for item in &args_list {
        let json =
            marshal::to_json(item).map_err(|e| runtime_err(&ctx, &format!("api_many: {e}")))?;
        batch.push(json);
    }
    let entries = send_batch(tx, &ctx, name, batch)?;
    // Never throws for a per-item outcome either — an array of exactly `batch.len()` maps, ok or
    // not, is the whole contract (`runtime::partial`'s "partial failure never hard-fails a run",
    // extended here to a script's own view of the same batch).
    marshal::to_dynamic(&Value::Array(entries)).map_err(|e| runtime_err(&ctx, &e.to_string()))
}

fn first_entry(
    ctx: &NativeCallContext,
    mut entries: Vec<Value>,
    caller: &str,
) -> Result<Value, Box<EvalAltResult>> {
    if entries.is_empty() {
        return Err(runtime_err(
            ctx,
            &format!("{caller}: empty reply from the host bridge"),
        ));
    }
    Ok(entries.remove(0))
}

/// `api`'s own error idiom: throw a catchable exception carrying the per-item error message. This
/// is the one function of the three that *does* throw on a per-item failure — `api_try`/
/// `api_many` never do, by design (see their own doc comments).
fn entry_to_value(ctx: &NativeCallContext, entry: Value) -> Result<Dynamic, Box<EvalAltResult>> {
    let ok = entry.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        let value = entry.get("value").cloned().unwrap_or(Value::Null);
        marshal::to_dynamic(&value).map_err(|e| runtime_err(ctx, &e.to_string()))
    } else {
        let message = entry
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("api call failed")
            .to_owned();
        Err(runtime_err(ctx, &message))
    }
}

fn runtime_err(ctx: &NativeCallContext, msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(msg.to_owned()),
        ctx.call_position(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_names_never_collide_with_a_rhai_reserved_name() {
        for name in BINDING_NAMES {
            assert!(
                !RESERVED_BY_RHAI.contains(name),
                "{name:?} is reserved by rhai's interpreter and would be silently shadowed"
            );
        }
    }
}
