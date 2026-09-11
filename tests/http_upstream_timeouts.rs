//! Integration tests proving a hang and a slowloris trickle both get cut off, never hung on —
//! split out from `tests/http_upstream.rs` purely to keep both files under the workspace's
//! 400-line cap (see that file's module docs).

mod fixture;

use std::time::Duration;

use api2mcp::http::{self, CallError, SsrfPolicy};
use fixture::harness::{bound_get, loopback_pool, send_params, service_for};
use fixture::{Behavior, Fixture};

#[tokio::test]
async fn a_hang_is_cut_off_by_the_deadline() {
    let f = Fixture::start().await;
    f.set("/hang", Behavior::Hang);

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let mut params = send_params(&service, &policy);
    params.deadline = Duration::from_millis(300);

    let url = service.base_url.join("/hang").expect("valid url");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        http::send(&client, bound_get(url), params),
    )
    .await
    .expect("send must not itself hang past the deadline");
    assert!(matches!(result.unwrap_err(), CallError::Timeout));
}

#[tokio::test]
async fn a_slowloris_trickle_is_cut_off_by_the_read_timeout() {
    let f = Fixture::start().await;
    f.set(
        "/trickle",
        Behavior::SlowlorisTrickle {
            chunk_bytes: vec![b'.'; 8],
            delay: Duration::from_millis(800),
            chunks: 50,
        },
    );

    // A short `timeout_ms` becomes both the connect *and* read timeout on the built client (see
    // `http::client::build_client`) — well under the trickle's 800 ms inter-chunk delay.
    let mut service = service_for(&f);
    service.timeout_ms = 150;
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/trickle").expect("valid url");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        http::send(&client, bound_get(url), params),
    )
    .await
    .expect("send must not itself hang");
    assert!(
        result.is_err(),
        "a stalled read must be cut off, not hung on"
    );
}
