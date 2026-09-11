//! `Engine::new_raw()` configured by hand: the eight sub-packages `StandardPackage` composes
//! minus `BasicTimePackage`, every resource limit set explicitly, `eval`/`import` disabled, and
//! `print`/`debug` routed to `tracing` at `DEBUG` so they never reach a model's context or a run's
//! audit body.
//!
//! **Why not `StandardPackage`.** `rhai/src/packages/pkg_std.rs` composes it as Core + BitField +
//! Logic + BasicMath + BasicArray + BasicBlob + BasicMap + **BasicTime** + MoreString.
//! `BasicTimePackage` registers `timestamp()` — a wall clock inside a script, an outright I7
//! break (`runtime::fanout`'s module docs: "no clock, no randomness"). `entanglement` registers
//! `StandardPackage`, which is correct for it and wrong here. The [`tests::registered_functions`]
//! golden test below is what keeps a future accidental `StandardPackage`/`BasicTimePackage`
//! addition from landing silently.

use std::time::Instant;

use rhai::packages::{
    BasicArrayPackage, BasicBlobPackage, BasicMapPackage, BasicMathPackage, BitFieldPackage,
    CorePackage, LogicPackage, MoreStringPackage, Package,
};
use rhai::{AST, Dynamic, Engine};

use super::errors::ScriptFailure;

const MAX_OPERATIONS: u64 = 2_000_000;
const MAX_CALL_LEVELS: usize = 64;
const MAX_EXPR_DEPTH: usize = 64;
const MAX_STRING_SIZE: usize = 256 * 1024;
const MAX_ARRAY_SIZE: usize = 10_000;
const MAX_MAP_SIZE: usize = 10_000;
const MAX_VARIABLES: usize = 1_000;
const MAX_FUNCTIONS: usize = 200;
const MAX_STRINGS_INTERNED: usize = 1_024;

/// Builds the sandboxed engine. `deadline` is a snapshot of the run's own wall-clock budget
/// (`BudgetMeter::remaining_time`, taken once before `spawn_blocking` — see `script::mod` for why
/// it can't cross as a live reference): `on_progress` trips as soon as it's passed, producing
/// `EvalAltResult::ErrorTerminated`, the one error class a script's `try`/`catch` cannot swallow.
/// `None` means the run has no wall-clock opinion at all; [`MAX_OPERATIONS`] is still a hard,
/// deterministic backstop regardless.
pub fn build_engine(deadline: Option<Instant>) -> Engine {
    let mut engine = Engine::new_raw();
    register_packages(&mut engine);
    apply_limits(&mut engine);

    engine.on_progress(move |_ops| {
        if deadline.is_some_and(|dl| Instant::now() >= dl) {
            Some(Dynamic::from("budget exceeded: wall_clock".to_string()))
        } else {
            None
        }
    });

    // `print`/`debug` must never reach the model's context or a run's audit body (I4-adjacent: a
    // script could otherwise use `print` as a side channel around projection/redaction). Routed
    // to `tracing::debug!` only.
    engine.on_print(|text| tracing::debug!(target: "api2mcp::script", "{text}"));
    engine.on_debug(|text, source, pos| {
        tracing::debug!(
            target: "api2mcp::script",
            source = source.unwrap_or(""),
            position = %pos,
            "{text}"
        );
    });

    engine
}

/// The eight `StandardPackage` sub-packages minus `BasicTimePackage` — see the module docs.
fn register_packages(engine: &mut Engine) {
    engine.register_global_module(CorePackage::new().as_shared_module());
    engine.register_global_module(BitFieldPackage::new().as_shared_module());
    engine.register_global_module(LogicPackage::new().as_shared_module());
    engine.register_global_module(BasicMathPackage::new().as_shared_module());
    engine.register_global_module(BasicArrayPackage::new().as_shared_module());
    engine.register_global_module(BasicBlobPackage::new().as_shared_module());
    engine.register_global_module(BasicMapPackage::new().as_shared_module());
    engine.register_global_module(MoreStringPackage::new().as_shared_module());
}

fn apply_limits(engine: &mut Engine) {
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_EXPR_DEPTH);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_SIZE);
    engine.set_max_map_size(MAX_MAP_SIZE);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_functions(MAX_FUNCTIONS);
    // No module ever resolves, even a well-formed one — combined with `disable_symbol("import")`
    // below this is belt-and-braces: `new_raw()` also starts with no module resolver at all.
    engine.set_max_modules(0);
    engine.set_max_strings_interned(MAX_STRINGS_INTERNED);
    // An absent map key errors instead of silently yielding `()` — a shape drift in an upstream
    // response becomes a failure a script's author sees, not a wrong answer they don't.
    engine.set_fail_on_invalid_map_property(true);
    engine.set_strict_variables(true);
    engine.disable_symbol("eval");
    engine.disable_symbol("import");
}

/// Compiles `source` once. `run_script` calls this exactly once per invocation — the plan's "the
/// AST is compiled once at load time, not per invocation" for this chunk's own scope; a future
/// resolve-level cache (mirroring `resolve::cache::PlanCache`) is what makes that true *across*
/// invocations of the same [`crate::model::ScriptDef`], and isn't this chunk's to build.
pub fn compile(engine: &Engine, source: &str) -> Result<AST, ScriptFailure> {
    engine
        .compile(source)
        .map_err(|e| ScriptFailure::from_parse_error(source, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One canary expression per registered package, plus the two `BasicTimePackage` functions
    /// that must be **absent**. `present` records whether the engine found *some* function
    /// matching the call's name+arity — not whether the call itself succeeded, so a probe's exact
    /// argument types don't need to be pixel-perfect, only its name and arity. Sorted by probe
    /// text so the golden file's diff is stable regardless of source edits to this list's order.
    ///
    /// This stands in for `Engine::gen_fn_signatures`, which needs the `metadata` Cargo feature —
    /// not enabled for this crate (`Cargo.toml` is out of this chunk's file ownership) — so this
    /// is a behavioral probe, not an introspection dump. It still does the one job that matters:
    /// any accidental package swap changes which names resolve, which changes this golden file.
    const PROBES: &[&str] = &[
        // CorePackage (LanguageCore + Arithmetic + BasicString + BasicIterator + BasicFn)
        "is_even(4)",
        "is_odd(4)",
        "is_zero(0)",
        "42.to_string()",
        "42.to_hex()",
        "(0..3).contains(1)",
        "let f = Fn(\"is_even\"); f.name()",
        // BitFieldPackage
        "5.get_bits(0, 2)",
        // LogicPackage
        "min(1, 2)",
        "max(1, 2)",
        // BasicMathPackage
        "floor(4.7)",
        "ceiling(4.2)",
        "round(4.5)",
        "fraction(4.5)",
        "atan(1.0)",
        "is_nan(1.0)",
        "parse_int(\"5\")",
        "to_int(4.9)",
        "to_float(4)",
        // BasicArrayPackage
        "[1, 2, 3].is_empty()",
        "[3, 1, 2].sort(); [3, 1, 2]",
        "[1, 2].index_of(2)",
        // BasicBlobPackage
        "blob(3, 0).is_empty()",
        "blob(3, 0).len()",
        // BasicMapPackage
        "let m = #{a: 1}; m.mixin(#{b: 2});",
        // MoreStringPackage
        "\"hello\".contains(\"ell\")",
        "\"hello\".ends_with(\"lo\")",
        "\"hello\".crop(1, 3)",
        "let s = \"hi\"; s.make_upper(); s",
        // BasicTimePackage — must be ABSENT. `timestamp()`'s absence is also its own dedicated
        // test below; kept here too so the golden file is the single place a reviewer sees the
        // whole registered/excluded picture at once.
        "timestamp()",
        "let t = timestamp(); t.elapsed()",
    ];

    fn probe(engine: &Engine, expr: &str) -> bool {
        match engine.eval::<Dynamic>(expr) {
            Ok(_) => true,
            Err(e) => !matches!(*e, rhai::EvalAltResult::ErrorFunctionNotFound(..)),
        }
    }

    #[test]
    fn registered_functions_match_the_committed_golden_file() {
        let engine = build_engine(None);
        let mut sorted: Vec<&str> = PROBES.to_vec();
        sorted.sort_unstable();

        let report: String = sorted
            .iter()
            .map(|expr| {
                format!(
                    "{}: {}\n",
                    if probe(&engine, expr) {
                        "present"
                    } else {
                        "absent "
                    },
                    expr
                )
            })
            .collect();

        let golden = include_str!("engine_probe.golden");
        assert_eq!(
            report, golden,
            "registered function set changed — if this is deliberate, update \
             src/script/engine_probe.golden; if not, a package registration regressed \
             (BasicTimePackage above all)"
        );
    }

    #[test]
    fn timestamp_is_an_unknown_function() {
        let engine = build_engine(None);
        let err = engine.eval::<Dynamic>("timestamp()").unwrap_err();
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorFunctionNotFound(..)),
            "expected function-not-found, got: {err}"
        );
    }

    #[test]
    fn eval_and_import_are_disabled() {
        let engine = build_engine(None);
        let _ = engine
            .eval::<Dynamic>(r#"eval("1")"#)
            .expect_err("eval must be disabled");
        let _ = engine
            .compile(r#"import "std" as s;"#)
            .expect_err("import must be disabled");
    }

    #[test]
    fn fail_on_invalid_map_property_and_strict_variables_are_set() {
        let engine = build_engine(None);
        let _ = engine
            .eval::<Dynamic>("let m = #{a: 1}; m.b")
            .expect_err("reading an absent map key must error, not yield ()");
        let _ = engine
            .eval::<Dynamic>("undeclared_variable")
            .expect_err("an undeclared variable must error under strict_variables");
    }

    #[test]
    fn operation_limit_terminates_a_runaway_loop() {
        let engine = build_engine(None);
        let err = engine
            .eval::<Dynamic>("let i = 0; loop { i += 1; }")
            .unwrap_err();
        assert!(matches!(
            *err,
            rhai::EvalAltResult::ErrorTooManyOperations(_)
        ));
    }

    #[test]
    fn wall_clock_deadline_terminates_and_is_not_catchable() {
        let engine = build_engine(Some(Instant::now()));
        let err = engine
            .eval::<Dynamic>(r#"try { let i = 0; loop { i += 1; } } catch(e) { 0 }"#)
            .unwrap_err();
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorTerminated(..)),
            "expected terminated-by-deadline, got: {err}"
        );
    }

    #[test]
    fn no_wall_clock_deadline_never_trips_on_its_own() {
        let engine = build_engine(None);
        // Bounded loop, well under MAX_OPERATIONS — must simply finish.
        let v = engine
            .eval::<i64>("let i = 0; while i < 100 { i += 1; } i")
            .unwrap();
        assert_eq!(v, 100);
    }

    #[test]
    fn compile_reports_the_syntax_errors_position() {
        let engine = build_engine(None);
        let source = "let x = 1;\nlet y = ;\n";
        let failure = compile(&engine, source).unwrap_err();
        assert_eq!(failure.line, Some(2));
    }
}
