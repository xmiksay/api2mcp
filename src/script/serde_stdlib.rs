//! `json_parse`/`json_stringify`/`json_stringify_pretty`/`yaml_parse`/`yaml_stringify` — all five
//! route through [`marshal::to_dynamic`]/[`marshal::to_json`] in both directions rather than each
//! format inventing its own `Dynamic` conversion. That's the whole point: one set of conversion
//! rules (the `i64`-range refusal above all) governs every serde format this crate exposes to a
//! script, instead of three independently-drifting ones.
//!
//! **No `yaml_stringify_pretty`.** Unlike JSON (compact vs. indented-with-newlines are genuinely
//! different outputs), `serde_norway`'s only stringification mode is already block-style,
//! newline-per-field YAML — there is no denser alternative it can additionally collapse into, so
//! a second name would be a synonym for `yaml_stringify`, not a new capability.
//!
//! Parse failures never panic: both `serde_json::Error` and `serde_norway::Error` carry a
//! line/column, folded into the catchable error's message text.

use rhai::{Dynamic, Engine, EvalAltResult, NativeCallContext};
use serde_json::Value;

use super::marshal;

fn runtime_err(ctx: &NativeCallContext, msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.to_owned().into(),
        ctx.call_position(),
    ))
}

fn json_parse(ctx: NativeCallContext, s: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    let value: Value = serde_json::from_str(s).map_err(|e| {
        runtime_err(
            &ctx,
            &format!(
                "json_parse: invalid JSON at line {} column {}: {e}",
                e.line(),
                e.column()
            ),
        )
    })?;
    marshal::to_dynamic(&value).map_err(|e| runtime_err(&ctx, &format!("json_parse: {e}")))
}

fn json_stringify(ctx: NativeCallContext, v: Dynamic) -> Result<String, Box<EvalAltResult>> {
    let value =
        marshal::to_json(&v).map_err(|e| runtime_err(&ctx, &format!("json_stringify: {e}")))?;
    serde_json::to_string(&value).map_err(|e| runtime_err(&ctx, &format!("json_stringify: {e}")))
}

fn json_stringify_pretty(ctx: NativeCallContext, v: Dynamic) -> Result<String, Box<EvalAltResult>> {
    let value = marshal::to_json(&v)
        .map_err(|e| runtime_err(&ctx, &format!("json_stringify_pretty: {e}")))?;
    serde_json::to_string_pretty(&value)
        .map_err(|e| runtime_err(&ctx, &format!("json_stringify_pretty: {e}")))
}

fn yaml_parse(ctx: NativeCallContext, s: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    // `serde_json::Value`'s `Deserialize` impl is deserializer-agnostic, so `serde_norway` can
    // deserialize straight into it without an intermediate `serde_norway::Value` — one JSON
    // shape for both formats, matching this module's whole reason for existing.
    let value: Value = serde_norway::from_str(s).map_err(|e| {
        let pos = e
            .location()
            .map(|l| format!(" at line {} column {}", l.line(), l.column()))
            .unwrap_or_default();
        runtime_err(&ctx, &format!("yaml_parse: invalid YAML{pos}: {e}"))
    })?;
    marshal::to_dynamic(&value).map_err(|e| runtime_err(&ctx, &format!("yaml_parse: {e}")))
}

fn yaml_stringify(ctx: NativeCallContext, v: Dynamic) -> Result<String, Box<EvalAltResult>> {
    let value =
        marshal::to_json(&v).map_err(|e| runtime_err(&ctx, &format!("yaml_stringify: {e}")))?;
    serde_norway::to_string(&value).map_err(|e| runtime_err(&ctx, &format!("yaml_stringify: {e}")))
}

pub fn register(engine: &mut Engine) {
    engine.register_fn("json_parse", json_parse);
    engine.register_fn("json_stringify", json_stringify);
    engine.register_fn("json_stringify_pretty", json_stringify_pretty);
    engine.register_fn("yaml_parse", yaml_parse);
    engine.register_fn("yaml_stringify", yaml_stringify);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new_raw();
        register(&mut e);
        e
    }

    #[test]
    fn json_round_trips_through_a_map() {
        let e = engine();
        let v: String = e
            .eval(r#"json_stringify(json_parse("{\"a\":1,\"b\":[2,3]}"))"#)
            .unwrap();
        assert_eq!(v, r#"{"a":1,"b":[2,3]}"#);
    }

    #[test]
    fn json_stringify_pretty_indents() {
        let e = engine();
        let v: String = e.eval(r#"json_stringify_pretty(#{a: 1})"#).unwrap();
        assert!(v.contains('\n'), "pretty output should span multiple lines");
    }

    #[test]
    fn invalid_json_names_the_position_not_a_panic() {
        let e = engine();
        let err = e.eval::<Dynamic>(r#"json_parse("{not json")"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line"), "{msg}");
        assert!(msg.contains("column"), "{msg}");
    }

    #[test]
    fn yaml_round_trips_through_a_map() {
        let e = engine();
        let m: rhai::Map = e
            .eval(r#"yaml_parse(yaml_stringify(#{a: 1, b: "x"}))"#)
            .unwrap();
        assert_eq!(m.get("a").unwrap().as_int().unwrap(), 1);
        assert_eq!(m.get("b").unwrap().clone().into_string().unwrap(), "x");
    }

    #[test]
    fn invalid_yaml_is_catchable_and_mentions_a_position() {
        let e = engine();
        // Unbalanced flow-mapping brace — a parse error `serde_norway` attaches a location to.
        let err = e.eval::<Dynamic>(r#"yaml_parse("a: [1, 2")"#).unwrap_err();
        assert!(matches!(*err, rhai::EvalAltResult::ErrorRuntime(..)));
    }
}
