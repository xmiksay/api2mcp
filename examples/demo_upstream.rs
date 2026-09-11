//! The upstream `examples/demo.pack.yaml` points at.
//!
//! The skeleton is meant to be demoable with no credentials and no network, which needs
//! *something* to curate. This is that something: a fixed, in-memory catalogue on
//! `127.0.0.1:8089`, matching the `base_url` and `origin_allowlist` in the demo pack.
//!
//! ```text
//! cargo run --example demo_upstream     # terminal 1
//! make seed && make run                 # terminal 2
//! ```
//!
//! It deliberately returns a wider shape than the pack's projection keeps — every item carries
//! `owner`, `secret_internal_id` and a `_links` block that `get-item`'s projection drops. That
//! is the whole point of the product: what reaches the model is the curated subset, not
//! whatever the upstream happened to send.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

const ADDR: &str = "127.0.0.1:8089";

fn catalogue() -> Vec<Value> {
    (1..=25)
        .map(|i| {
            json!({
                "id": i.to_string(),
                "title": format!("Item {i}"),
                "owner": { "username": format!("user{}", i % 4), "id": 900 + i },
                "secret_internal_id": format!("internal-{i:08}"),
                "_links": { "self": format!("/items/{i}"), "html": format!("/ui/items/{i}") },
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
}

async fn list_items(Query(q): Query<ListQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    let items: Vec<Value> = catalogue().into_iter().take(limit).collect();
    Json(json!({ "items": items, "total": 25 }))
}

async fn get_item(Path(id): Path<String>) -> impl IntoResponse {
    match catalogue().into_iter().find(|i| i["id"] == json!(id)) {
        Some(item) => Json(item).into_response(),
        // A real 404, so the non-2xx path is demoable too: ask for item 999 and the tool
        // reports an http_status failure rather than projecting this body as if it were data.
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": format!("no item with id {id:?}") })),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/items", get(list_items))
        .route("/items/{id}", get(get_item));

    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    println!("demo upstream listening on http://{ADDR}");
    println!("  GET /items?limit=N");
    println!("  GET /items/{{id}}   (999 gives a 404, on purpose)");
    axum::serve(listener, app).await?;
    Ok(())
}
