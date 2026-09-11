//! Integration tests for `script::dates` exercised through `run_script`, mirroring
//! `tests/script.rs`'s own pattern (see that file's module docs).

use std::time::Duration;

use api2mcp::model::Budgets;

use crate::fixture::{Fixture, harness::slug};
use crate::support::{plan_with_script, run, script_def};

#[tokio::test]
async fn a_timestamp_returned_from_a_script_lands_as_an_rfc3339_string() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"parse_timestamp("2024-03-05T12:30:00Z")"#);

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    assert_eq!(value, serde_json::json!("2024-03-05T12:30:00+00:00"));
}

#[tokio::test]
async fn execution_start_is_constant_within_one_run_but_differs_across_runs() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"let a = execution_start(); let b = execution_start(); [a == b, a.to_rfc3339()]"#,
    );

    let first = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("first run succeeds");
    let first_arr = first.as_array().expect("array result");
    assert_eq!(first_arr[0], serde_json::json!(true));

    // A real wall-clock tick between the two runs' own `BudgetMeter::new` calls, so their
    // `execution_start()` values are guaranteed distinct rather than landing in the same instant
    // by coincidence.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let second = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("second run succeeds");
    let second_arr = second.as_array().expect("array result");

    assert_ne!(
        first_arr[1], second_arr[1],
        "two separate BudgetMeters must see two different execution_start() values"
    );
}

#[tokio::test]
async fn rfc3339_and_format_based_parsing_support_arithmetic_and_comparison() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let a = parse_timestamp("2024-01-01T00:00:00Z");
        let b = parse_date("02/01/2024 00:00", "%d/%m/%Y %H:%M");
        let sum = a + days(1);
        [sum == b, (b - a).whole_hours() == 24, a < b, b.is_between(a, a + days(2))]
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("script succeeds");
    let arr = value.as_array().expect("array result");
    assert_eq!(
        *arr,
        vec![
            serde_json::json!(true),
            serde_json::json!(true),
            serde_json::json!(true),
            serde_json::json!(true),
        ]
    );
}

#[tokio::test]
async fn validation_returns_false_on_unparseable_input_instead_of_throwing() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"[is_valid_timestamp("not a date"), is_valid_date("not a date", "%Y-%m-%d"), is_valid_date("2024-01-01", "%Y-%m-%d")]"#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("validation never throws");
    assert_eq!(
        value,
        serde_json::json!([false, false, true]),
        "unparseable input yields false, not an exception"
    );
}
