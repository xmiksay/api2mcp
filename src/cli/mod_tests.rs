//! Unit tests for `cli/mod.rs` — split out purely to keep that file under the workspace's
//! 400-line cap, mirroring `call.rs`'s identical `#[path = "call_tests.rs"]` split.

use super::*;
use serde_json::json;

#[test]
fn args_parse_as_json_with_a_string_fallback() {
    let got = parse_args(&["n=3".into(), "s=hello".into(), "b=true".into()]).unwrap();
    assert_eq!(got["n"], json!(3));
    assert_eq!(got["s"], json!("hello"));
    assert_eq!(got["b"], json!(true));
}

#[test]
fn a_value_containing_equals_keeps_its_tail() {
    let got = parse_args(&["q=a=b".into()]).unwrap();
    assert_eq!(got["q"], json!("a=b"));
}

#[test]
fn retyping_narrows_a_number_onto_a_string_param() {
    use crate::model::{Param, ParamLocation, ParamType};
    let p = Param {
        name: "id".into(),
        location: ParamLocation::Path,
        ty: ParamType::String,
        required: true,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position: 0,
    };
    let mut args = parse_args(&["id=3".into()]).expect("parses");
    assert_eq!(args["id"], json!(3), "parse_args guesses a number");
    retype_args_for_params(&[p], &mut args);
    assert_eq!(args["id"], json!("3"), "retyped to the declared string");
}

#[test]
fn retyping_leaves_a_non_string_param_alone() {
    use crate::model::{Param, ParamLocation, ParamType};
    let p = Param {
        name: "limit".into(),
        location: ParamLocation::Query,
        ty: ParamType::Integer,
        required: false,
        default: None,
        fixed: None,
        enum_values: None,
        description: None,
        position: 0,
    };
    let mut args = parse_args(&["limit=10".into()]).expect("parses");
    retype_args_for_params(&[p], &mut args);
    assert_eq!(
        args["limit"],
        json!(10),
        "an integer param keeps its number"
    );
}

#[test]
fn a_missing_equals_is_an_error() {
    assert!(parse_args(&["oops".into()]).is_err());
}

// `resolve_user` is what the CLI problem statement is entirely about: `call`/`script
// run`/`import`/`export` have no session to infer an owner from, so `--user` (defaulting to
// the sole user when exactly one exists) has to be the answer — and a multi-user box must
// never have that default guess for it. Database-backed, so these live here as unit tests
// rather than under `tests/`, matching `store::test_support`'s own reasoning.
mod resolve_user_tests {
    use super::*;
    use crate::store::test_support::ScratchDb;
    use crate::store::{NewUser, Stores};

    #[tokio::test]
    async fn defaults_to_the_sole_user_when_user_is_absent() {
        let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(scratch.db.clone());
        let created = stores
            .user()
            .create(NewUser {
                email: "sole-user@example.com".into(),
                password: "correct horse battery staple".into(),
            })
            .await
            .expect("create user");

        let resolved = resolve_user(&stores, None).await.expect("resolves");
        assert_eq!(resolved.id, created.id);

        scratch.teardown().await.expect("teardown");
    }

    #[tokio::test]
    async fn errors_clearly_when_no_user_exists() {
        let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(scratch.db.clone());

        let err = resolve_user(&stores, None)
            .await
            .expect_err("no users exist yet");
        assert!(format!("{err:#}").contains("no users exist"));

        scratch.teardown().await.expect("teardown");
    }

    #[tokio::test]
    async fn errors_clearly_when_ambiguous_and_user_is_absent() {
        let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(scratch.db.clone());
        for email in ["alice@example.com", "bob@example.com"] {
            stores
                .user()
                .create(NewUser {
                    email: email.into(),
                    password: "correct horse battery staple".into(),
                })
                .await
                .expect("create user");
        }

        let err = resolve_user(&stores, None)
            .await
            .expect_err("ambiguous without --user");
        assert!(format!("{err:#}").contains("more than one user exists"));

        scratch.teardown().await.expect("teardown");
    }

    #[tokio::test]
    async fn works_without_disambiguation_when_user_is_given_even_with_several_accounts() {
        let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let stores = Stores::new(scratch.db.clone());
        let alice = stores
            .user()
            .create(NewUser {
                email: "alice2@example.com".into(),
                password: "correct horse battery staple".into(),
            })
            .await
            .expect("create user");
        stores
            .user()
            .create(NewUser {
                email: "bob2@example.com".into(),
                password: "correct horse battery staple".into(),
            })
            .await
            .expect("create user");

        let resolved = resolve_user(&stores, Some("alice2@example.com"))
            .await
            .expect("resolves by explicit --user");
        assert_eq!(resolved.id, alice.id);

        scratch.teardown().await.expect("teardown");
    }
}
