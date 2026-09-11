//! Integration tests for `script::run_script` against the hermetic fixture upstream (see
//! `tests/fixture`). Mirrors `tests/runtime_budgets.rs`'s own pattern: `EndpointPlan`, the
//! `DispatchContext`/`ConcurrencyLimits`/`BudgetMeter` triple, and `loopback_pool()`/`service_for`
//! are all constructed by hand rather than through `resolve::build_plan` — this crate's script
//! layer is pure over those hand-built types and doesn't need a database to exercise. No test
//! here needs `TEST_DATABASE_URL`; it's mentioned in the chunk brief only because the harness
//! module is shared crate-wide, not because anything below touches Postgres.
//!
//! Plan/script builders live in `tests/script/support.rs`, split out purely to keep this file
//! under the workspace's 400-line cap (mirroring `tests/fixture/harness.rs`'s own reason for
//! existing next to `tests/fixture/mod.rs`).

mod fixture;
#[path = "script/support.rs"]
mod support;

use std::time::Duration;

use api2mcp::model::{Budgets, Param, ParamLocation, ParamType};
use api2mcp::script::{RunScriptError, ScriptFailureKind};

use fixture::harness::slug;
use fixture::{Behavior, Fixture};
use support::{plan_with_script, run, script_def};

#[tokio::test]
async fn api_calls_the_declared_alias_and_returns_the_projected_value() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::Json(serde_json::json!({"n": 1})));
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"api("item", #{id: "1"})"#);

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    assert_eq!(value, serde_json::json!({"n": 1}));
}

#[tokio::test]
async fn api_throws_a_catchable_error_on_an_upstream_failure() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::ExactBody { len: 5, byte: b'x' });
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    // `try`/`catch` around `api()`'s own per-item failure is exactly the idiom it must support —
    // this is not a budget/termination case. `try`/`catch` is a Rhai *statement*, not an
    // expression, so the outcome is captured into `result` rather than relied on as the block's
    // own value.
    let script = script_def(
        r#"
        let result = "";
        try {
            result = api("item", #{id: "1"});
        } catch(e) {
            result = "caught";
        }
        result
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("caught, so the script still succeeds");
    assert_eq!(value, serde_json::json!("caught"));
}

#[tokio::test]
async fn api_many_results_land_in_input_order_under_reversed_completion_order() {
    let f = Fixture::start().await;
    // Item 0 answers slowest, item 4 answers fastest — completion order is the exact reverse of
    // input order, and the assembled array must still read 0..5 by input index.
    for i in 0..5u32 {
        let delay = Duration::from_millis(((5 - i) * 15) as u64);
        f.set(
            &format!("/items/{i}"),
            Behavior::SlowlorisTrickle {
                chunk_bytes: serde_json::to_vec(&serde_json::json!({"n": i})).unwrap(),
                delay,
                chunks: 1,
            },
        );
    }
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let ids = [];
        for i in 0..5 { ids.push(#{id: i.to_string()}); }
        api_many("item", ids)
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    let entries = value.as_array().expect("array result");
    assert_eq!(entries.len(), 5);
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry["index"], serde_json::json!(i));
        assert_eq!(entry["ok"], serde_json::json!(true));
        assert_eq!(entry["value"], serde_json::json!({"n": i}));
    }
}

#[tokio::test]
async fn api_many_partial_failure_yields_exactly_batch_len_entries_with_correct_indices() {
    let f = Fixture::start().await;
    for i in 0..4u32 {
        if i == 2 {
            f.set(
                &format!("/items/{i}"),
                Behavior::ExactBody { len: 5, byte: b'x' },
            );
        } else {
            f.set(
                &format!("/items/{i}"),
                Behavior::Json(serde_json::json!({"n": i})),
            );
        }
    }
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let ids = [];
        for i in 0..4 { ids.push(#{id: i.to_string()}); }
        api_many("item", ids)
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("api_many never throws for a per-item failure");
    let entries = value.as_array().expect("array result");
    assert_eq!(entries.len(), 4, "exactly batch.len() entries, never fewer");

    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry["index"], serde_json::json!(i));
        if i == 2 {
            assert_eq!(entry["ok"], serde_json::json!(false));
            assert!(entry.get("value").is_none(), "value absent when !ok");
            // The error is an object carrying a stable `kind` a script can branch on, not a
            // message — branching on prose is not something a script author should have to do.
            assert!(entry["error"]["kind"].is_string(), "error.kind present");
            assert!(
                entry["error"]["message"].is_string(),
                "error.message present"
            );
        } else {
            assert_eq!(entry["ok"], serde_json::json!(true));
            assert!(entry.get("error").is_none(), "error absent when ok");
            assert_eq!(entry["value"], serde_json::json!({"n": i}));
        }
    }
}

#[tokio::test]
async fn api_try_matches_the_api_many_element_shape_for_a_single_call() {
    let f = Fixture::start().await;
    f.set("/items/9", Behavior::ExactBody { len: 5, byte: b'x' });
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"api_try("item", #{id: "9"})"#);

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("api_try never throws");
    assert_eq!(value["ok"], serde_json::json!(false));
    assert_eq!(value["index"], serde_json::json!(0));
    assert!(value["error"]["kind"].is_string(), "error.kind present");
    assert!(
        value["error"]["message"].is_string(),
        "error.message present"
    );
}

#[tokio::test]
async fn fail_on_invalid_map_property_forces_checking_ok_before_reading_value() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::ExactBody { len: 5, byte: b'x' });
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    // Reading `.value` on a failed `api_try` entry must itself error — `set_fail_on_invalid_map_
    // property` plus "value absent iff !ok" together force the author to check `ok` first.
    let script = script_def(r#"let r = api_try("item", #{id: "1"}); r.value"#);

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("reading .value on a failed entry must error");
    match err {
        RunScriptError::Script(f) => assert_eq!(f.kind, ScriptFailureKind::Runtime),
        other => panic!("expected a script failure, got {other:?}"),
    }
}

#[tokio::test]
async fn an_undeclared_name_is_refused_i1() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"api("not-declared", #{})"#);

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("an undeclared name must never reach HTTP");
    match err {
        RunScriptError::Script(f) => {
            assert_eq!(f.kind, ScriptFailureKind::Runtime);
            assert!(f.message.contains("not declared"), "{}", f.message);
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
    assert!(
        f.seen().is_empty(),
        "an undeclared name must never dispatch"
    );
}

#[tokio::test]
async fn a_batch_over_the_static_cap_is_refused_without_attempting_any_call() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let ids = [];
        for i in 0..501 { ids.push(#{id: i.to_string()}); }
        api_many("item", ids)
        "#,
    );

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("a batch over the static cap is a script-authoring bug");
    match err {
        RunScriptError::Script(f) => assert_eq!(f.kind, ScriptFailureKind::Runtime),
        other => panic!("expected a script failure, got {other:?}"),
    }
    assert!(
        f.seen().is_empty(),
        "an oversized batch must never dispatch"
    );
}

#[tokio::test]
async fn import_and_eval_are_refused_and_timestamp_is_unknown() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);

    for source in [r#"eval("1")"#, r#"import "std" as s;"#, "timestamp()"] {
        let err = run(&plan, Budgets::default(), &script_slug, &script_def(source))
            .await
            .expect_err(&format!("{source:?} must be refused"));
        assert!(matches!(err, RunScriptError::Script(_)), "{source:?}");
    }
}

#[tokio::test]
async fn wall_clock_budget_terminates_a_runaway_script_uncatchably() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let budgets = Budgets {
        wall_clock: Some(Duration::from_millis(50)),
        ..Budgets::default()
    };
    // Wrapped in its own `try`/`catch` — the whole point of the test is that this does not help.
    let script = script_def(r#"try { let i = 0; loop { i += 1; } } catch(e) { "caught" }"#);

    let err = run(&plan, budgets, &script_slug, &script)
        .await
        .expect_err("the wall-clock budget must terminate the run");
    match err {
        RunScriptError::Script(failure) => {
            assert_eq!(failure.kind, ScriptFailureKind::Terminated);
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_syntax_error_reports_the_correct_line_and_column() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def("let x = 1;\nlet y = ;\n");

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("malformed source must not compile");
    match err {
        RunScriptError::Script(failure) => {
            assert_eq!(failure.kind, ScriptFailureKind::Compile);
            assert_eq!(failure.line, Some(2));
            assert!(failure.snippet.is_some());
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_runtime_error_reports_the_line_of_the_offending_call() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def("let x = 1;\napi(\"not-declared\", #{});\n");

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("undeclared name must error");
    match err {
        RunScriptError::Script(failure) => {
            assert_eq!(failure.kind, ScriptFailureKind::Runtime);
            assert_eq!(failure.line, Some(2));
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
}

#[tokio::test]
async fn caller_side_argument_mismatch_is_distinct_from_a_script_failure() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let mut script = script_def("1");
    script.params = vec![Param {
        name: "required_arg".to_owned(),
        location: ParamLocation::Local,
        ty: ParamType::String,
        required: true,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position: 0,
    }];

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("a missing required param must be refused before the script even runs");
    assert!(matches!(err, RunScriptError::Args(_)));
}
