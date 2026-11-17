//! Integration tests for how `()` crosses the script boundary in both directions: an optional
//! script param the caller omitted is bound as `()` (issue #8), and a `()` a script passes to an
//! omittable api_call param means "omitted" — for a script only, never a direct caller's `null`.

use api2mcp::model::{Budgets, Param, ParamLocation, ParamType};
use api2mcp::runtime::dispatch::DispatchError;
use api2mcp::schema::ValidationError;
use api2mcp::script::{RunScriptError, ScriptFailureKind};

use crate::fixture::{Behavior, Fixture, harness::slug};
use crate::support::{dispatch_direct, plan_with_script, run, script_def};

fn optional_param(name: &str) -> Param {
    Param {
        name: name.to_owned(),
        location: ParamLocation::Local,
        ty: ParamType::String,
        required: false,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position: 0,
    }
}

#[tokio::test]
async fn an_omitted_optional_param_is_bound_as_unit() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let mut script = script_def(r#"if type_of(s) == "()" { "omitted" } else { s }"#);
    script.params = vec![optional_param("s")];

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("an omitted optional param must still compile and run");
    assert_eq!(value, serde_json::json!("omitted"));
}

#[tokio::test]
async fn strict_variables_still_reject_a_typo_of_a_declared_param() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let mut script = script_def("type_of(ss)");
    script.params = vec![optional_param("s")];

    let err = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect_err("an undeclared name must still fail compilation");
    match err {
        RunScriptError::Script(failure) => assert_eq!(failure.kind, ScriptFailureKind::Compile),
        other => panic!("expected a compile failure, got {other:?}"),
    }
}

#[tokio::test]
async fn unit_for_an_optional_api_call_param_is_omitted_from_the_request() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::Json(serde_json::json!({"n": 1})));
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(
        r#"
        let one = api("item", #{id: "1", q: ()});
        let many = api_many("item", [#{id: "1", q: ()}]);
        let tried = api_try("item", #{id: "1", q: ()});
        [one, many[0].ok, tried.ok]
        "#,
    );

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("() for an optional param is not a validation error");
    assert_eq!(value, serde_json::json!([{"n": 1}, true, true]));
    let seen = f.seen();
    assert_eq!(seen.len(), 3);
    for req in seen {
        // `limit`'s default is always sent; `q` (no default) must not be.
        assert_eq!(req.raw_path_and_query, "/items/1?limit=10");
    }
}

#[tokio::test]
async fn unit_for_an_optional_api_call_param_with_a_default_sends_the_default() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::Json(serde_json::json!({"n": 1})));
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"api("item", #{id: "1", limit: ()})"#);

    run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("() for a defaulted param reads as omission");
    let seen = f.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].raw_path_and_query, "/items/1?limit=10");
}

#[tokio::test]
async fn a_direct_callers_null_for_an_optional_param_is_still_a_type_error() {
    let f = Fixture::start().await;
    f.set("/items/1", Behavior::Json(serde_json::json!({"n": 1})));
    let plan = plan_with_script(&f, &slug("demo-script"));

    let err = dispatch_direct(plan, serde_json::json!({"id": "1", "q": null}))
        .await
        .expect_err("only a script's () reads as omission");
    assert!(
        matches!(
            err,
            DispatchError::Args(ValidationError::WrongType { ref param, .. }) if param == "q"
        ),
        "{err:?}"
    );
    assert!(f.seen().is_empty(), "a refused call must never dispatch");
}

#[tokio::test]
async fn unit_for_a_required_api_call_param_is_still_a_validation_error() {
    let f = Fixture::start().await;
    let script_slug = slug("demo-script");
    let plan = plan_with_script(&f, &script_slug);
    let script = script_def(r#"api_try("item", #{id: ()})"#);

    let value = run(&plan, Budgets::default(), &script_slug, &script)
        .await
        .expect("api_try never throws");
    assert_eq!(value["ok"], serde_json::json!(false));
    assert_eq!(value["error"]["kind"], serde_json::json!("args"));
    assert!(f.seen().is_empty(), "a refused call must never dispatch");
}
