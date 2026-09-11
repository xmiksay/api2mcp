//! Drives a paginated `ApiCall` to completion (or to the page cap) by repeatedly calling
//! [`send::send`]. Every subsequent page's URL goes through `send`'s own [`check_url`] call
//! exactly like a redirect hop does — `paginate` never constructs a request that skips it.
//!
//! Hitting the page cap is an error, never a silent stop: quietly returning page 1 of 50 when
//! the caller asked for "all of it" is a wrong answer, not a partial one.
//!
//! [`check_url`]: super::ssrf::check_url

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::model::Pagination;

use super::bind::BoundRequest;
use super::send::{CallError, CallResponse, SendParams, send};

#[derive(Debug, Clone, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum PaginateError {
    #[error("call: {0}")]
    Call(#[from] CallError),
    #[error("reached the {max}-page cap without pagination completing")]
    PageCapExceeded { max: u32 },
    #[error("page response body was not valid JSON: {message}")]
    ResponseNotJson { message: String },
    #[error("next-page cursor at {pointer:?} matched a {actual} value, expected a JSON string")]
    CursorNotString {
        pointer: String,
        actual: &'static str,
    },
}

/// Fetches pages starting from `first` until [`Pagination`] reports no next page, or until
/// `max_pages` is reached (an error, per the module docs). Returns every page fetched, in order.
/// `params` is reused unchanged for every page's `send` call — `SendParams` is `Copy` precisely
/// so pagination can do this without a factory closure.
pub async fn paginate(
    client: &reqwest::Client,
    first: BoundRequest,
    pagination: &Pagination,
    max_pages: u32,
    params: SendParams<'_>,
) -> Result<Vec<CallResponse>, PaginateError> {
    let mut pages = Vec::new();
    let mut request = Some(first);

    while let Some(current) = request.take() {
        if pages.len() as u32 >= max_pages {
            return Err(PaginateError::PageCapExceeded { max: max_pages });
        }
        let response = send(client, current, params).await?;
        request = next_request(&response, pagination)?;
        pages.push(response);
    }
    Ok(pages)
}

fn next_request(
    response: &CallResponse,
    pagination: &Pagination,
) -> Result<Option<BoundRequest>, PaginateError> {
    let Pagination::Cursor {
        next_cursor_path,
        query_param,
    } = pagination
    else {
        return Ok(None);
    };

    let body: Value =
        serde_json::from_slice(&response.body).map_err(|e| PaginateError::ResponseNotJson {
            message: e.to_string(),
        })?;

    // A pointer that doesn't resolve means "no next page" — the same terminal signal as an
    // explicit JSON `null` at that pointer. Anything else that isn't a string is a shape the
    // definer didn't mean, so it's an error rather than a guess.
    let cursor = match next_cursor_path.resolve(&body) {
        Ok(Value::Null) | Err(_) => return Ok(None),
        Ok(Value::String(s)) => s.clone(),
        Ok(other) => {
            return Err(PaginateError::CursorNotString {
                pointer: next_cursor_path.to_string(),
                actual: json_type_name(other),
            });
        }
    };

    let next_url = set_query_param(&response.url, query_param, &cursor);
    Ok(Some(BoundRequest {
        method: ::http::Method::GET,
        url: next_url,
        headers: std::collections::BTreeMap::new(),
        body: None,
    }))
}

/// Replaces (or adds) `key` in `url`'s query string, leaving every other pair untouched and in
/// its original relative order (I7) — `url::Url::query_pairs_mut` only appends, so pairs are
/// collected, filtered, and rewritten from scratch.
fn set_query_param(url: &url::Url, key: &str, value: &str) -> url::Url {
    let mut out = url.clone();
    let kept: Vec<(String, String)> = out
        .query_pairs()
        .filter(|(k, _)| k != key)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    out.query_pairs_mut()
        .clear()
        .extend_pairs(kept.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .append_pair(key, value);
    out
}

/// The JSON type name of `value`, for error messages. Duplicated from (the private)
/// `schema::coerce::json_type_name` rather than reused — that module is `mod coerce;` (not `pub
/// mod`) inside `schema`, so it isn't reachable from here without editing a file this chunk
/// doesn't own; see the chunk report for the follow-up to make it `pub(crate)` and shared.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_query_param_replaces_an_existing_key_and_keeps_others() {
        let url = url::Url::parse("https://api.example.com/x?page=1&sort=asc").expect("valid");
        let out = set_query_param(&url, "page", "2");
        assert_eq!(out.as_str(), "https://api.example.com/x?sort=asc&page=2");
    }

    #[test]
    fn set_query_param_adds_a_missing_key() {
        let url = url::Url::parse("https://api.example.com/x").expect("valid");
        let out = set_query_param(&url, "cursor", "abc");
        assert_eq!(out.as_str(), "https://api.example.com/x?cursor=abc");
    }
}
