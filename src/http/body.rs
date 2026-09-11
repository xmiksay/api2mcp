//! Streams a response body with a running byte counter, aborting the moment it exceeds `cap`.
//!
//! Byte accounting happens **after decompression**: the resource being defended is our own
//! memory, and a 1 KB gzip bomb inflating to 1 GB is the actual attack, so the bomb must never be
//! materialised. `reqwest`'s automatic decompression (the `gzip`/`brotli`/`zstd` features) runs
//! transparently underneath `bytes_stream()`, so every chunk this module ever sees is already
//! decompressed — counting them is counting the real cost.
//!
//! The wire `Content-Length` check below is a cheap early reject only, not the defence: with
//! decompression enabled, `Response::content_length()` is `None` for a compressed response (the
//! header is stripped once a recognised `Content-Encoding` triggers decoding), so this can only
//! ever catch an oversized *uncompressed* response before a single byte is read. Being over cap
//! is always an error, never a silent truncation — a truncated JSON body is a parse failure at
//! best and a wrong answer at worst.

use futures::StreamExt;
use reqwest::Response;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum BodyError {
    #[error(
        "response body is {actual} bytes, which exceeds the {cap}-byte cap (from Content-Length)"
    )]
    ContentLengthExceedsCap { cap: u64, actual: u64 },
    #[error("response body exceeded the {cap}-byte cap while streaming")]
    StreamExceedsCap { cap: u64 },
    #[error("reading response body: {message}")]
    Transport { message: String },
}

/// Reads `response`'s body into memory, erroring the instant the running (decompressed) byte
/// count would exceed `cap`. `cap` of `0` is legal and simply rejects any non-empty body.
pub async fn read_capped(response: Response, cap: u64) -> Result<Vec<u8>, BodyError> {
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(BodyError::ContentLengthExceedsCap { cap, actual: len });
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BodyError::Transport {
            message: super::redact::redact_message(&e.to_string()),
        })?;
        if buf.len() as u64 + chunk.len() as u64 > cap {
            return Err(BodyError::StreamExceedsCap { cap });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    // The behaviours that actually exercise decompression need a real HTTP response, which
    // means a real listener — see `tests/http_upstream.rs` (`gzip_bomb_trips_the_cap_on_...` and
    // friends) for those. This module's own unit tests are limited to what's reachable with no
    // network: the error type's shape.
    use super::*;

    #[test]
    fn errors_serialize_with_a_stable_tag() {
        let err = BodyError::StreamExceedsCap { cap: 10 };
        let json = serde_json::to_value(&err).expect("serializes");
        assert_eq!(json["error"], "stream_exceeds_cap");
        assert_eq!(json["cap"], 10);
    }
}
