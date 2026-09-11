//! Integration tests for `http::paginate`, against the same hermetic upstream as
//! `tests/http_upstream.rs` (split into its own file purely to keep both under the workspace's
//! 400-line cap — see that file's module docs).

mod fixture;

use api2mcp::http::{self, PaginateError, SsrfPolicy};
use api2mcp::model::Pagination;
use fixture::harness::{bound_get, loopback_pool, send_params, service_for};
use fixture::{Behavior, Fixture};

fn cursor_pagination() -> Pagination {
    Pagination::Cursor {
        next_cursor_path: jsonptr::PointerBuf::from_tokens(["next"]),
        query_param: "cursor".to_owned(),
    }
}

#[tokio::test]
async fn pagination_forwards_the_cursor_from_one_page_into_the_next_requests_query() {
    let f = Fixture::start().await;
    // Always reports the same next cursor — enough to observe page 2's query string; the page
    // cap (reached here, since this fixture never naturally terminates) has its own dedicated
    // test below.
    f.set(
        "/page",
        Behavior::Json(serde_json::json!({"items": [1], "next": "abc"})),
    );

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/page").expect("valid url");
    let err = http::paginate(&client, bound_get(url), &cursor_pagination(), 2, params)
        .await
        .unwrap_err();
    assert!(matches!(err, PaginateError::PageCapExceeded { max: 2 }));

    let seen = f.seen();
    assert_eq!(seen.len(), 2);
    assert!(
        !seen[0].raw_path_and_query.contains("cursor="),
        "the first page must not carry a cursor param: {}",
        seen[0].raw_path_and_query
    );
    assert!(
        seen[1].raw_path_and_query.contains("cursor=abc"),
        "the cursor from page 1's body must be forwarded into page 2's query: {}",
        seen[1].raw_path_and_query
    );
}

#[tokio::test]
async fn pagination_stops_with_an_error_at_the_page_cap_not_a_silent_partial_result() {
    let f = Fixture::start().await;
    // Always reports a next page — never naturally terminates.
    f.set(
        "/forever",
        Behavior::Json(serde_json::json!({"next": "again"})),
    );

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/forever").expect("valid url");
    let pagination = cursor_pagination();

    let err = http::paginate(&client, bound_get(url), &pagination, 3, params)
        .await
        .unwrap_err();
    assert!(matches!(err, PaginateError::PageCapExceeded { max: 3 }));
    assert_eq!(
        f.seen().len(),
        3,
        "exactly the cap's worth of pages were attempted"
    );
}

#[tokio::test]
async fn pagination_terminates_on_an_explicit_null_cursor() {
    let f = Fixture::start().await;
    f.set("/last", Behavior::Json(serde_json::json!({"next": null})));

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/last").expect("valid url");
    let pages = http::paginate(&client, bound_get(url), &cursor_pagination(), 10, params)
        .await
        .expect("terminates normally");
    assert_eq!(pages.len(), 1);
    assert_eq!(f.seen().len(), 1);
}
