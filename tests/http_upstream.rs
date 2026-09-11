//! Integration tests for `http::{send, body, resolver, client, auth}` against a real, hermetic
//! upstream (`tests/fixture`). No test here needs network access: everything talks to
//! `127.0.0.1`, and DNS-dependent scenarios use `StaticDns`/`RebindDns` instead of a real
//! resolver.
//!
//! Pagination and timeouts get their own sibling files, `tests/http_upstream_pagination.rs` and
//! `tests/http_upstream_timeouts.rs` — the natural split that keeps every file under the
//! workspace's 400-line cap.

mod fixture;

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;

use api2mcp::http::{
    self, BindError, BodyError, CallError, GuardError, SsrfPolicy, StaticDns, UpstreamPool,
};
use api2mcp::model::{AuthKind, AuthProvider, Origin, Service};
use fixture::harness::{bound_get, loopback_pool, send_params, service_for, slug};
use fixture::{Behavior, Fixture};

// ---------------------------------------------------------------------------------------------
// Byte cap / gzip bomb
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn gzip_bomb_trips_the_cap_on_decompressed_bytes_not_wire_length() {
    let f = Fixture::start().await;
    // ~103 KB on the wire (see `fixture::gzip_bomb`'s docs on why not the plan's literal
    // "100 KB/100 MB": no compression-encoding dependency is available to this chunk), inflating
    // to ~16 MiB decompressed.
    let repeats = 65_028;
    let decompressed_len = fixture::gzip_bomb::decompressed_len(repeats);
    assert!(decompressed_len > 16 * 1024 * 1024);
    f.set(
        "/bomb",
        Behavior::GzipBody {
            byte: b'A',
            repeats,
        },
    );

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");

    let mut params = send_params(&service, &policy);
    params.max_response_bytes = 1024 * 1024; // 1 MiB cap, far below the ~16 MiB decompressed size

    let url = service.base_url.join("/bomb").expect("valid url");
    let err = http::send(&client, bound_get(url), params)
        .await
        .expect_err("must be capped");

    // Specifically `StreamExceedsCap`, not `ContentLengthExceedsCap` — proving the streaming,
    // decompressed-byte cap is what caught this, not the (absent, for a gzip response) wire
    // Content-Length.
    assert!(
        matches!(err, CallError::Body(BodyError::StreamExceedsCap { .. })),
        "expected a streaming cap error, got {err:?}"
    );
}

#[tokio::test]
async fn an_oversized_uncompressed_body_is_rejected_by_the_cheap_content_length_check() {
    let f = Fixture::start().await;
    f.set(
        "/big",
        Behavior::ExactBody {
            len: 2 * 1024 * 1024,
            byte: b'x',
        },
    );

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");

    let mut params = send_params(&service, &policy);
    params.max_response_bytes = 1024 * 1024;

    let url = service.base_url.join("/big").expect("valid url");
    let err = http::send(&client, bound_get(url), params)
        .await
        .expect_err("must be capped");
    assert!(matches!(
        err,
        CallError::Body(BodyError::ContentLengthExceedsCap { .. })
    ));
}

#[tokio::test]
async fn a_body_within_the_cap_round_trips_intact() {
    let f = Fixture::start().await;
    f.set(
        "/ok",
        Behavior::ExactBody {
            len: 1024,
            byte: b'z',
        },
    );

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/ok").expect("valid url");
    let response = http::send(&client, bound_get(url), params)
        .await
        .expect("ok");
    assert_eq!(response.status, ::http::StatusCode::OK);
    assert_eq!(response.body.len(), 1024);
    assert!(response.body.iter().all(|&b| b == b'z'));
}

// ---------------------------------------------------------------------------------------------
// Redirects: SSRF re-check per hop, and auth never crossing origins
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_redirect_to_a_metadata_ip_is_refused() {
    let f = Fixture::start().await;
    f.set(
        "/go",
        Behavior::Redirect {
            status: 302,
            location: "http://169.254.169.254/latest/meta-data".to_owned(),
        },
    );

    let mut service = service_for(&f);
    // The metadata origin is deliberately *allowlisted* here — this test isolates the
    // literal-IP SSRF check specifically (I2's plain origin-allowlist check would refuse an
    // unlisted origin regardless, which is covered by `http::origin`'s own unit tests; this one
    // proves the redirect hop re-runs `check_url`'s IP-classification half too).
    service.origin_allowlist.insert(
        Origin::of(&url::Url::parse("http://169.254.169.254/").expect("valid url"))
            .expect("valid origin"),
    );
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/go").expect("valid url");
    let err = http::send(&client, bound_get(url), params)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CallError::Guard(GuardError::DeniedLiteralIp { .. })),
        "expected a denied-literal-ip guard error, got {err:?}"
    );
    // Only the first hop was ever attempted.
    assert_eq!(f.seen().len(), 1);
}

#[tokio::test]
async fn a_redirect_to_a_different_allowlisted_origin_does_not_carry_the_first_origins_auth() {
    let origin_a = Fixture::start().await;
    let origin_b = Fixture::start().await;

    let redirect_target = format!("{}redirected", origin_b.base_url());
    origin_a.set(
        "/start",
        Behavior::Redirect {
            status: 302,
            location: redirect_target,
        },
    );
    origin_b.set(
        "/redirected",
        Behavior::Json(serde_json::json!({"ok": true})),
    );

    let origin_a_id = Origin::of(&origin_a.base_url()).expect("valid origin");
    let origin_b_id = Origin::of(&origin_b.base_url()).expect("valid origin");

    // One service definition whose declared reach spans both origins (both are, after all,
    // where its api_calls actually resolve, in this test) — the redirect target only needs to be
    // in the *allowlist* to be followed at all; whether the credential comes along is I5's job,
    // decided by `bound_origin` below, not by allowlist membership.
    let mut service = service_for(&origin_a);
    service.origin_allowlist = BTreeSet::from([origin_a_id.clone(), origin_b_id]);

    let provider = AuthProvider {
        slug: slug("demo-auth"),
        service_slug: service.slug.clone(),
        kind: AuthKind::StaticHeader,
        credential_env_key: "A2M_TEST_HTTP_UPSTREAM_REDIRECT_AUTH".to_owned(),
        header_name: "Authorization".to_owned(),
        value_template: "Bearer ".to_owned(),
        scopes: Vec::new(),
        token_url: None,
        bound_origin: origin_a_id,
    };
    unsafe { std::env::set_var("A2M_TEST_HTTP_UPSTREAM_REDIRECT_AUTH", "top-secret") };

    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let mut params = send_params(&service, &policy);
    params.auth = Some(&provider);

    let url = service.base_url.join("/start").expect("valid url");
    let response = http::send(&client, bound_get(url), params)
        .await
        .expect("ok");
    unsafe { std::env::remove_var("A2M_TEST_HTTP_UPSTREAM_REDIRECT_AUTH") };

    assert_eq!(response.status, ::http::StatusCode::OK);

    let a_requests = origin_a.seen();
    let b_requests = origin_b.seen();
    assert_eq!(a_requests.len(), 1, "only the initial hop hit origin A");
    assert_eq!(b_requests.len(), 1, "the redirect hop hit origin B");

    assert!(
        a_requests[0].headers.contains_key("authorization"),
        "the same-origin first hop must still carry its credential"
    );
    assert!(
        !b_requests[0].headers.contains_key("authorization"),
        "the cross-origin redirect hop must never carry origin A's credential"
    );
}

// ---------------------------------------------------------------------------------------------
// Proxy environment variables must not change where the request goes
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn http_proxy_env_var_does_not_change_the_destination() {
    let f = Fixture::start().await;
    f.set("/direct", Behavior::Json(serde_json::json!({"ok": true})));

    // A bogus, unreachable "proxy" — if `.no_proxy()` weren't in effect, reqwest would try to
    // dial this instead of the fixture and the request would fail, not succeed.
    unsafe { std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1") };

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = service.base_url.join("/direct").expect("valid url");
    let result = http::send(&client, bound_get(url), params).await;

    unsafe { std::env::remove_var("HTTP_PROXY") };

    let response = result.expect("the proxy env var must not have been honored");
    assert_eq!(response.status, ::http::StatusCode::OK);
    assert_eq!(f.seen().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// DNS rebinding / poisoning: the guard runs on every resolve, not just a pre-flight check
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn static_dns_pointing_the_service_hostname_at_a_metadata_ip_fails_at_connect() {
    // A fixture stands in for "the real destination" — proving it never receives anything is
    // only meaningful if reaching it was otherwise on the table, so the service's hostname is
    // deliberately mapped (as if poisoned) to the metadata IP instead of the fixture's real
    // address.
    let f = Fixture::start().await;
    f.set("/data", Behavior::Json(serde_json::json!({"secret": true})));

    let bad_dns = StaticDns::new().with("spoofed.test", vec![IpAddr::from([169, 254, 169, 254])]);
    let pool = UpstreamPool::new(Arc::new(bad_dns), SsrfPolicy::default());

    let base_url: url::Url = format!("http://spoofed.test:{}", f.addr.port())
        .parse()
        .expect("valid url");
    let origin = Origin::of(&base_url).expect("valid origin");
    let service = Service {
        slug: slug("spoofed"),
        base_url: base_url.clone(),
        origin_allowlist: BTreeSet::from([origin]),
        default_headers: Default::default(),
        timeout_ms: 2_000,
        max_concurrency: 4,
        rate_limit_per_min: None,
        max_response_bytes: 1024 * 1024,
    };
    let policy = SsrfPolicy::default();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    let url = base_url.join("/data").expect("valid url");
    let err = http::send(&client, bound_get(url), params)
        .await
        .unwrap_err();

    // The failure must come from resolution-time filtering, not a socket-level connect error —
    // proving the guard, not the OS, is what stopped this.
    let message = err.to_string();
    assert!(
        message.contains("survived the SSRF filter") || message.contains("resolving"),
        "expected a DNS-guard error, got: {message}"
    );
    assert!(
        f.seen().is_empty(),
        "the real fixture must never see this request"
    );
}

// ---------------------------------------------------------------------------------------------
// Percent-escapes: the whole reason this fixture records the *raw* path
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn percent_escapes_in_the_path_reach_the_fixture_intact() {
    let f = Fixture::start().await;
    f.set("/a%2Fb", Behavior::Status(204));

    let service = service_for(&f);
    let policy = SsrfPolicy {
        allow_loopback: true,
    };
    let pool = loopback_pool();
    let client = pool.client_for(&service).await.expect("client");
    let params = send_params(&service, &policy);

    // Constructed directly (not through `http::bind`, which would refuse `%2F` as a path
    // param) — this is checking the fixture's own recording fidelity, and the transport's
    // handling of an already-escaped URL, not the binder.
    let url = url::Url::parse(&format!("{}a%2Fb", service.base_url)).expect("valid url");
    let response = http::send(&client, bound_get(url), params)
        .await
        .expect("ok");
    assert_eq!(response.status, ::http::StatusCode::NO_CONTENT);
    assert_eq!(f.seen()[0].raw_path_and_query, "/a%2Fb");
}

/// A regression guard: `BindError`'s variants are still reachable through `http::` re-exports
/// after C4's additions to `http::mod`'s public surface (a compile-time check, not a runtime
/// assertion).
#[allow(dead_code)]
fn _bind_error_type_is_still_exported(_e: BindError) {}
