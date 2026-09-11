//! `ApiError`: the HTTP-error shape for anything served over a plain axum route (as opposed to
//! `server::mcp`'s JSON-RPC framing, which never uses this type — see its own module doc for why
//! it builds `JsonRpcResponse::error` directly instead).
//!
//! **Every message reaching a client passes through [`crate::http::redact_message`] first.** A
//! [`crate::store::StoreError`] never carries a raw `DbErr` (`store::db_err` already strips that
//! at the source — see its module doc), but redacting again here is what makes "no internal
//! detail reaches a client" a property of this one conversion point rather than a discipline
//! every route handler has to remember on its own.
//!
//! Wired into every route in `server::api` (chunk C14, the read-write admin JSON API) — this
//! type's first real caller.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::http::redact_message;
use crate::store::StoreError;

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Forbidden(String),
    /// Every failure `pack::validate` found against the definitions as they would look *after*
    /// a pending write — plural because the whole point of validating before persisting is
    /// reporting every problem in one pass, not stopping at the first one found (see
    /// `server::api::validate_write`).
    Validation(Vec<String>),
    /// A store/database failure, or anything else that must never describe itself to a client —
    /// the message is always the fixed string below, never `err.to_string()`.
    Internal,
}

impl ApiError {
    /// Maps a [`StoreError`] to the right client-facing shape. `NotFound`/`Conflict` already
    /// carry a message safe to show as-is (see `StoreError`'s own doc); everything else collapses
    /// to [`ApiError::Internal`] rather than forwarding a message that was only ever meant for
    /// `tracing::error!`.
    pub fn from_store(err: StoreError) -> Self {
        match err {
            StoreError::NotFound => ApiError::NotFound("not found".to_owned()),
            StoreError::Conflict(message) => ApiError::BadRequest(message),
            StoreError::Malformed(_) | StoreError::Db | StoreError::Internal(_) => {
                ApiError::Internal
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let errors = match self {
            ApiError::Validation(errors) => errors,
            ApiError::NotFound(m) => return single(StatusCode::NOT_FOUND, &m),
            ApiError::BadRequest(m) => return single(StatusCode::BAD_REQUEST, &m),
            ApiError::Forbidden(m) => return single(StatusCode::FORBIDDEN, &m),
            ApiError::Internal => {
                return single(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
        let redacted: Vec<String> = errors.iter().map(|e| redact_message(e)).collect();
        (StatusCode::BAD_REQUEST, Json(json!({ "errors": redacted }))).into_response()
    }
}

fn single(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": redact_message(message) }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_maps_to_404() {
        let response = ApiError::NotFound("no such endpoint".into()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn store_not_found_maps_to_api_not_found() {
        assert!(matches!(
            ApiError::from_store(StoreError::NotFound),
            ApiError::NotFound(_)
        ));
    }

    #[test]
    fn store_conflict_maps_to_bad_request_preserving_the_message() {
        match ApiError::from_store(StoreError::Conflict("duplicate slug".into())) {
            ApiError::BadRequest(m) => assert_eq!(m, "duplicate slug"),
            other => panic!("expected BadRequest, got a differently-shaped ApiError: {other:?}"),
        }
    }

    #[test]
    fn store_db_error_never_reaches_the_client_as_a_message() {
        let response = ApiError::from_store(StoreError::Db).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn validation_reports_every_error_at_once_under_the_errors_key() {
        let response = ApiError::Validation(vec!["first problem".into(), "second problem".into()])
            .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("reading the response body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let errors = value["errors"].as_array().expect("errors array");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0], "first problem");
        assert_eq!(errors[1], "second problem");
    }

    #[tokio::test]
    async fn an_embedded_url_in_the_message_is_redacted() {
        let response =
            ApiError::BadRequest("rejected https://user:pass@example.com/x?token=y".to_owned())
                .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("reading the response body");
        let text = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("token=y"));
        assert!(text.contains("https://example.com/x"));
    }
}
