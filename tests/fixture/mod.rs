//! A real `axum::serve` upstream, listening on `127.0.0.1:0`, for `tests/http_upstream.rs`. It
//! has to be a real listener: the code path under test runs through `reqwest`, the guarded
//! resolver, and the manual redirect loop, none of which `tower::Service::oneshot` can drive.
//!
//! Every request is recorded as a [`SeenRequest`] with the **raw** path-and-query — straight off
//! `http::Uri`, which never percent-decodes — so a test can assert on percent-escapes surviving
//! intact (the I3 bug class a decoded-path assertion would silently hide).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::Router;
use serde_json::Value;

#[path = "gzip_bomb.rs"]
pub mod gzip_bomb;

#[path = "harness.rs"]
pub mod harness;

/// One request the fixture received.
#[derive(Debug, Clone)]
pub struct SeenRequest {
    pub method: String,
    /// Path plus (if present) `?query`, exactly as `http::Uri` parsed it — percent-escapes
    /// intact, never decoded.
    pub raw_path_and_query: String,
    /// Header names lower-cased; values as sent, or `"<non-utf8>"` for a value that isn't valid
    /// UTF-8 text.
    pub headers: BTreeMap<String, String>,
}

/// What the fixture does when a request matches a registered path.
#[derive(Clone)]
pub enum Behavior {
    Json(Value),
    Status(u16),
    /// A plain body of exactly `len` bytes, all equal to `byte` — no `Content-Encoding`.
    ExactBody {
        len: usize,
        byte: u8,
    },
    /// A gzip body whose wire bytes are tiny but whose decompressed size is
    /// `gzip_bomb::decompressed_len(repeats)` — see that module for how.
    GzipBody {
        byte: u8,
        repeats: usize,
    },
    /// Never responds. The client's own deadline/timeout is what ends this.
    Hang,
    /// Streams `chunks` copies of `chunk_bytes`, sleeping `delay` before each — a slowloris that
    /// trickles bytes just fast enough to avoid looking dead, slow enough to trip a read timeout.
    SlowlorisTrickle {
        chunk_bytes: Vec<u8>,
        delay: Duration,
        chunks: usize,
    },
    Redirect {
        status: u16,
        location: String,
    },
    /// Fails (with `Status(fail_status)`) on the `n`th request to this path (1-indexed), and
    /// otherwise defers to `then`.
    FailNthRequest {
        n: u32,
        fail_status: u16,
        counter: Arc<AtomicU32>,
        then: Box<Behavior>,
    },
}

impl Behavior {
    pub fn fail_nth(n: u32, fail_status: u16, then: Behavior) -> Self {
        Behavior::FailNthRequest {
            n,
            fail_status,
            counter: Arc::new(AtomicU32::new(0)),
            then: Box::new(then),
        }
    }
}

struct FixtureState {
    seen: Mutex<Vec<SeenRequest>>,
    routes: Mutex<BTreeMap<String, Behavior>>,
}

/// A running fixture server. Dropping this leaves the server task running for the remainder of
/// the test process (each `#[tokio::test]` gets its own runtime, torn down — and with it every
/// task the test spawned — when the test function returns), which is fine for test-scoped use.
pub struct Fixture {
    pub addr: SocketAddr,
    state: Arc<FixtureState>,
}

impl Fixture {
    pub async fn start() -> Self {
        let state = Arc::new(FixtureState {
            seen: Mutex::new(Vec::new()),
            routes: Mutex::new(BTreeMap::new()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has a local address");

        let app: Router = Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture server run loop");
        });

        Self { addr, state }
    }

    /// Registers (or replaces) the behavior for an exact path (e.g. `"/items"`), ignoring query
    /// string.
    pub fn set(&self, path: &str, behavior: Behavior) {
        self.state
            .routes
            .lock()
            .expect("fixture routes lock")
            .insert(path.to_owned(), behavior);
    }

    pub fn seen(&self) -> Vec<SeenRequest> {
        self.state.seen.lock().expect("fixture seen lock").clone()
    }

    pub fn base_url(&self) -> url::Url {
        format!("http://127.0.0.1:{}", self.addr.port())
            .parse()
            .expect("fixture base url")
    }
}

async fn handle(State(state): State<Arc<FixtureState>>, req: Request) -> Response {
    let method = req.method().to_string();
    let uri = req.uri().clone();
    let mut raw_path_and_query = uri.path().to_owned();
    if let Some(query) = uri.query() {
        raw_path_and_query.push('?');
        raw_path_and_query.push_str(query);
    }
    let headers: BTreeMap<String, String> = req
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or("<non-utf8>").to_owned(),
            )
        })
        .collect();

    state
        .seen
        .lock()
        .expect("fixture seen lock")
        .push(SeenRequest {
            method,
            raw_path_and_query,
            headers,
        });

    let behavior = state
        .routes
        .lock()
        .expect("fixture routes lock")
        .get(uri.path())
        .cloned();
    match behavior {
        Some(b) => render(b).await,
        None => Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("building a response never fails for a fixed status+empty body"),
    }
}

fn respond(status: axum::http::StatusCode, body: Body) -> Response {
    Response::builder()
        .status(status)
        .body(body)
        .expect("building a response never fails for a fixed status+body")
}

async fn render(behavior: Behavior) -> Response {
    match behavior {
        Behavior::Json(value) => {
            let bytes = serde_json::to_vec(&value).expect("serializing a test fixture body");
            Response::builder()
                .status(axum::http::StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(bytes))
                .expect("building a json response")
        }
        Behavior::Status(code) => {
            let status =
                axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::OK);
            respond(status, Body::empty())
        }
        Behavior::ExactBody { len, byte } => {
            respond(axum::http::StatusCode::OK, Body::from(vec![byte; len]))
        }
        Behavior::GzipBody { byte, repeats } => Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_ENCODING, "gzip")
            .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(gzip_bomb::gzip_repeated_byte(byte, repeats)))
            .expect("building a gzip response"),
        Behavior::Hang => {
            // Long enough to never legitimately elapse in a test; the client's own
            // deadline/read-timeout is what's actually under test here.
            tokio::time::sleep(Duration::from_secs(3600)).await;
            respond(axum::http::StatusCode::OK, Body::empty())
        }
        Behavior::SlowlorisTrickle {
            chunk_bytes,
            delay,
            chunks,
        } => {
            let stream = futures::stream::unfold(0usize, move |i| {
                let chunk_bytes = chunk_bytes.clone();
                async move {
                    if i >= chunks {
                        return None;
                    }
                    tokio::time::sleep(delay).await;
                    Some((Ok::<_, std::io::Error>(chunk_bytes), i + 1))
                }
            });
            respond(axum::http::StatusCode::OK, Body::from_stream(stream))
        }
        Behavior::Redirect { status, location } => {
            let status =
                axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::FOUND);
            Response::builder()
                .status(status)
                .header(axum::http::header::LOCATION, location)
                .body(Body::empty())
                .expect("building a redirect response")
        }
        Behavior::FailNthRequest {
            n,
            fail_status,
            counter,
            then,
        } => {
            let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == n {
                Box::pin(render(Behavior::Status(fail_status))).await
            } else {
                Box::pin(render(*then)).await
            }
        }
    }
}
