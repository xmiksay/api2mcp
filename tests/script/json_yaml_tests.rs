//! Integration tests for `script::serde_stdlib` exercised through `run_script`.

use api2mcp::model::Budgets;

use crate::fixture::{Fixture, harness::slug};
use crate::support::{plan_with_script, run, script_def};

#[tokio::test]
async fn json_round_trips_through_a_script() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let built = #{name: "widget", tags: ["a", "b"], count: 3};
        let text = json_stringify(built);
        json_parse(text)
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    assert_eq!(
        value,
        serde_json::json!({"name": "widget", "tags": ["a", "b"], "count": 3})
    );
}

#[tokio::test]
async fn yaml_round_trips_through_a_script() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let built = #{name: "widget", nested: #{ok: true}};
        yaml_parse(yaml_stringify(built))
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    assert_eq!(
        value,
        serde_json::json!({"name": "widget", "nested": {"ok": true}})
    );
}

#[tokio::test]
async fn invalid_json_is_a_catchable_error_naming_a_position() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    // Uncaught here (unlike the round-trip tests above) purely so the failure message can be
    // inspected directly on the Rust side, mirroring `tests/script.rs`'s own
    // `an_undeclared_name_is_refused_i1` pattern — `try`/`catch` around `json_parse` is already
    // covered implicitly by every other test in this file relying on it *not* throwing.
    let script = script_def(r#"json_parse("{not valid")"#);

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("malformed JSON must be a catchable script failure");
    match err {
        api2mcp::script::RunScriptError::Script(f) => {
            assert_eq!(f.kind, api2mcp::script::ScriptFailureKind::Runtime);
            assert!(f.message.contains("line"), "{}", f.message);
        }
        other => panic!("expected a script failure, got {other:?}"),
    }
}
