//! I4's other structural half: [`crate::secret::Secret`] makes a credential un-formattable; this
//! module is the one place allowed to turn a header map, a URL, or a free-text message into
//! something safe to persist in a `runs`/`run_calls` row or return in an error body. The audit
//! recorder (C7) and the error mapper (`server/error`) must go through these — never format
//! anything upstream-derived themselves.

use std::collections::BTreeMap;

/// Header names whose values are always redacted, regardless of `HeaderValue::is_sensitive` —
/// defence in depth for anything that reaches here despite `HEADER_PARAM_ALLOWLIST` never
/// admitting a caller-chosen header by one of these names (e.g. an auth header merged in by
/// `http::send`'s `apply_auth`, C4).
const ALWAYS_REDACT: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "proxy-authorization",
];

const REDACTED: &str = "<redacted>";

/// Redacts a header map for persistence or return. A header is redacted if any of its values
/// were marked `set_sensitive(true)` (how a credential written by `apply_auth` shows up here) or
/// if its name is in [`ALWAYS_REDACT`].
pub fn redact_headers(headers: &::http::HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for name in headers.keys() {
        let lower = name.as_str().to_ascii_lowercase();
        let sensitive = ALWAYS_REDACT.contains(&lower.as_str())
            || headers
                .get_all(name)
                .iter()
                .any(::http::HeaderValue::is_sensitive);
        let rendered = if sensitive {
            REDACTED.to_owned()
        } else {
            headers
                .get_all(name)
                .iter()
                .map(|v| v.to_str().unwrap_or("<non-utf8>"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.insert(name.as_str().to_owned(), rendered);
    }
    out
}

/// Strips userinfo, query, and fragment from a URL before it's persisted or returned — a query
/// string can carry an API key (`?api_key=...`), and userinfo is a credential by definition.
pub fn redact_url(url: &url::Url) -> String {
    let mut clone = url.clone();
    let _ = clone.set_username("");
    let _ = clone.set_password(None);
    clone.set_query(None);
    clone.set_fragment(None);
    clone.to_string()
}

/// Redacts any whitespace-delimited token in `message` that parses as an `http`/`https` URL,
/// via [`redact_url`]. A conservative heuristic for free-text error strings (e.g. a raw
/// `reqwest` error) that might embed a URL with userinfo or a query-string credential — it does
/// not attempt to find a URL embedded mid-word, and it normalises whitespace to single spaces.
pub fn redact_message(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| match url::Url::parse(word) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => redact_url(&url),
            _ => word.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_headers_by_name() {
        let mut headers = ::http::HeaderMap::new();
        headers.insert(
            ::http::header::AUTHORIZATION,
            ::http::HeaderValue::from_static("Bearer secret-token"),
        );
        headers.insert(
            ::http::header::ACCEPT,
            ::http::HeaderValue::from_static("application/json"),
        );

        let redacted = redact_headers(&headers);
        assert_eq!(
            redacted.get("authorization").map(String::as_str),
            Some(REDACTED)
        );
        assert_eq!(
            redacted.get("accept").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn redacts_a_header_value_marked_sensitive_regardless_of_name() {
        let mut headers = ::http::HeaderMap::new();
        let mut value = ::http::HeaderValue::from_static("super-secret");
        value.set_sensitive(true);
        headers.insert("x-custom-token", value);

        let redacted = redact_headers(&headers);
        assert_eq!(
            redacted.get("x-custom-token").map(String::as_str),
            Some(REDACTED)
        );
    }

    #[test]
    fn redact_url_strips_userinfo_query_and_fragment() {
        let url = url::Url::parse("https://user:pass@api.example.com/path?token=secret#frag")
            .expect("valid url");
        assert_eq!(redact_url(&url), "https://api.example.com/path");
    }

    #[test]
    fn redact_url_leaves_a_clean_url_untouched() {
        let url = url::Url::parse("https://api.example.com/path").expect("valid url");
        assert_eq!(redact_url(&url), "https://api.example.com/path");
    }

    #[test]
    fn redact_message_redacts_an_embedded_url() {
        let msg = "upstream error calling https://user:pass@api.example.com/x?token=y timed out";
        let redacted = redact_message(msg);
        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("token=y"));
        assert!(redacted.contains("https://api.example.com/x"));
    }

    #[test]
    fn redact_message_passes_through_plain_text() {
        assert_eq!(
            redact_message("connection reset by peer"),
            "connection reset by peer"
        );
    }
}
