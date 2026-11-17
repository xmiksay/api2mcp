//! Integration tests for how `()` crosses the script boundary in both directions: an optional
//! script param the caller omitted is bound as `()` (issue #8), and a `()` a script passes to an
//! optional api_call param means "omitted".

use api2mcp::model::{Budgets, Param, ParamLocation, ParamType};
use api2mcp::script::{RunScriptError, ScriptFailureKind};

use crate::fixture::{Behavior, Fixture, harness::slug};
use crate::support::{plan_with_script, run, script_def};

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
        assert_eq!(req.raw_path_and_query, "/items/1", "q must not be sent");
    }
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
