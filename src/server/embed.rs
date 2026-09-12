//! The embedded Vue SPA: `#[derive(RustEmbed)]` bakes `web/dist` into the binary, and
//! [`spa_handler`] serves it with `knowledge-base`'s cache-header shape — hashed `assets/`
//! files are immutable forever, `index.html` is `no-store` (it references chunk names that
//! change on every deploy), and a miss under `/api/*` is a `404`, never the SPA shell, so a
//! typo'd or not-yet-built API route fails loudly instead of silently returning HTML.
//!
//! `build.rs` guarantees `web/dist` exists (a placeholder `index.html` on a clean clone with no
//! Node) — see the crate's `build.rs` doc — so this `#[folder = "web/dist"]` never fails to
//! compile.

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct SpaAssets;

/// Assets served regardless of whether the SPA was ever built.
///
/// The server-rendered auth pages need CSS, and it cannot come from either of the two obvious
/// places: an inline `<style>` block is blocked by the `default-src 'self'` CSP, and the SPA's
/// Tailwind bundle does not exist on a fresh clone or in CI, where `web/dist` is a build.rs
/// placeholder. Embedding a small stylesheet separately keeps the auth flow working with no
/// dependency on the frontend having been built — which is the same reason those pages are
/// server-rendered in the first place.
#[derive(RustEmbed)]
#[folder = "src/server/static"]
struct StaticAssets;

/// Serves [`StaticAssets`] under `/static`. Content-addressed by hand rather than by hash, so it
/// is cached for an hour rather than forever — long enough to matter, short enough that a change
/// ships without anyone thinking about cache busting.
pub async fn static_handler(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let Some(file) = StaticAssets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

/// The SPA fallback handler: serves a hashed static asset with a forever cache, `index.html`
/// (or any other non-file route) with `no-store`, and a `404` for any path under `/api/` that
/// didn't already match a real route — the one thing this handler must never do for such a path
/// is fall through to the SPA shell, which would look like a working response while actually
/// being a wrong one.
pub async fn spa_handler(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');

    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Some(content) = SpaAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Vite hashes filenames under assets/, so those are safe to cache forever; index.html
        // must never take this branch (see the fallback below).
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=3600"
        };
        return (
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            content.data.to_vec(),
        )
            .into_response();
    }

    serve_index()
}

fn serve_index() -> Response {
    match SpaAssets::get("index.html") {
        Some(content) => (
            [
                (header::CONTENT_TYPE, "text/html".to_string()),
                (
                    header::CACHE_CONTROL,
                    "no-store, must-revalidate".to_string(),
                ),
            ],
            content.data.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_auth_stylesheet_is_embedded() {
        // The auth pages link this rather than inlining a <style> block, because the CSP is
        // `default-src 'self'` with no `style-src 'unsafe-inline'` — an inline block is silently
        // blocked, which is what once left those pages rendering unstyled. If this asset ever
        // stops being embedded they break the same way, quietly.
        assert!(super::StaticAssets::get("auth.css").is_some());
    }

    use super::*;
    use axum::body::Body;

    fn get(path: &str) -> Request {
        Request::builder().uri(path).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn an_api_path_miss_is_404_not_the_spa_shell() {
        let response = spa_handler(get("/api/does-not-exist")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bare_api_path_is_also_404() {
        let response = spa_handler(get("/api")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_non_api_path_falls_back_to_the_spa_shell() {
        let response = spa_handler(get("/some/client/route")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store, must-revalidate")
        );
    }
}
