//! Integration tests for `script::regex_lib` exercised through `run_script`. The
//! compiled-once-per-pattern claim is proven at the Rust unit-test level instead
//! (`script::regex_lib::tests::get_or_compile_returns_the_same_compiled_pattern_on_repeat_calls`),
//! not here — timing an rhai loop would be flaky.

use api2mcp::model::Budgets;
use api2mcp::script::{RunScriptError, ScriptFailureKind};

use crate::fixture::{Fixture, harness::slug};
use crate::support::{plan_with_script, run, script_def};

#[tokio::test]
async fn captures_include_numbered_and_named_groups() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let caps = regex_captures("(?P<year>\\d{4})-(?P<month>\\d{2})-(\\d{2})", "2024-03-05");
        #{full: caps["0"], year: caps.year, month: caps.month, day: caps["3"]}
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    assert_eq!(
        value,
        serde_json::json!({"full": "2024-03-05", "year": "2024", "month": "03", "day": "05"})
    );
}

#[tokio::test]
async fn an_invalid_pattern_is_a_clean_catchable_error() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"regex_is_match("(unclosed", "text")"#);

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("an invalid pattern must be refused");
    match err {
        RunScriptError::Script(f) => {
            assert_eq!(f.kind, ScriptFailureKind::Runtime);
            assert!(!f.message.is_empty());
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
}

#[tokio::test]
async fn no_match_yields_unit_not_an_exception() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"type_of(regex_find("zzz", "abc"))"#);

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("no match is not an error");
    assert_eq!(value, serde_json::json!("()"));
}
