//! Credential material that cannot be formatted.
//!
//! I4 restated: "a credential has no path to a `String` that reaches a model-visible surface."
//! [`Secret`] makes that a compile-time property rather than a discipline to remember: it has no
//! `Debug` (a hand-written one prints only `Secret(<redacted>)`), no `Display`, no `Serialize`,
//! and no `Deref<Target = str>` — there is no expression that turns a `Secret` into a `String`
//! or `&str` by accident. The one deliberate exit is [`Secret::into_header_value`], which writes
//! straight into an `http::HeaderValue` marked `set_sensitive(true)`, so even `http`'s own
//! `Debug` for a `HeaderMap` redacts it downstream (see `http::redact` for the rest of I4).
//!
//! `tests/ui/` holds the `trybuild` compile-fail cases that pin this down:
//! `format!("{}", secret)` and `serde_json::to_string(&secret)` must not compile.

use std::env;
use std::fmt;

use serde::Serialize;
use zeroize::Zeroizing;

/// A credential value read from the environment. See the module docs for what's deliberately
/// *not* implemented on this type.
pub struct Secret(Zeroizing<String>);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Errors from loading or rendering a credential. Model-visible (a misconfigured
/// `credential_env_key` surfaces as a tool-call error), so `thiserror` + `Serialize` rather than
/// `anyhow` — and notably, neither variant can carry the credential's own value, only the env
/// var *name* or a fact about its shape.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum CredError {
    #[error("environment variable {env_key:?} is not set")]
    MissingEnvVar { env_key: String },
    #[error("credential value cannot be represented as an HTTP header value")]
    InvalidHeaderValue,
}

impl Secret {
    /// Reads the credential value from the environment variable named `env_key`. The name
    /// itself is never a secret — it's `AuthProvider::credential_env_key`, a plain DB column —
    /// only the value it points at is.
    pub fn load(env_key: &str) -> Result<Secret, CredError> {
        let value = env::var(env_key).map_err(|_| CredError::MissingEnvVar {
            env_key: env_key.to_owned(),
        })?;
        Ok(Secret(Zeroizing::new(value)))
    }

    /// The one exit from this type: renders `{prefix}{value}` (e.g. `prefix = "Bearer "`) into
    /// an `http::HeaderValue` marked sensitive. `pub(crate)` — the only intended caller is
    /// `http::send`'s `apply_auth` (C4); nothing in C3 calls this outside of tests, which is why
    /// this carries `#[allow(dead_code)]` until C4 wires up the real caller.
    #[allow(dead_code)]
    pub(crate) fn into_header_value(self, prefix: &str) -> Result<::http::HeaderValue, CredError> {
        let rendered = format!("{prefix}{}", self.0.as_str());
        let mut value =
            ::http::HeaderValue::from_str(&rendered).map_err(|_| CredError::InvalidHeaderValue)?;
        value.set_sensitive(true);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env vars are process-global state shared across test threads, so each test claims a
    // unique key rather than reusing e.g. "TEST_TOKEN" — a shared name would make these tests
    // flaky under `cargo test`'s default parallelism.

    #[test]
    fn load_reads_the_named_env_var() {
        // SAFETY: single-threaded within this test's own env var, unique key avoids collision.
        unsafe { env::set_var("A2M_TEST_SECRET_LOAD_OK", "sh-secret-value") };
        let secret = Secret::load("A2M_TEST_SECRET_LOAD_OK").expect("var is set");
        let header = secret.into_header_value("Bearer ").expect("valid header");
        assert_eq!(header.to_str().expect("ascii"), "Bearer sh-secret-value");
        unsafe { env::remove_var("A2M_TEST_SECRET_LOAD_OK") };
    }

    #[test]
    fn load_errors_when_env_var_is_absent() {
        let err = Secret::load("A2M_TEST_SECRET_DEFINITELY_UNSET").unwrap_err();
        assert_eq!(
            err,
            CredError::MissingEnvVar {
                env_key: "A2M_TEST_SECRET_DEFINITELY_UNSET".to_owned()
            }
        );
    }

    #[test]
    fn debug_never_prints_the_value() {
        unsafe { env::set_var("A2M_TEST_SECRET_DEBUG", "should-never-appear") };
        let secret = Secret::load("A2M_TEST_SECRET_DEBUG").expect("var is set");
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "Secret(<redacted>)");
        assert!(!rendered.contains("should-never-appear"));
        unsafe { env::remove_var("A2M_TEST_SECRET_DEBUG") };
    }

    #[test]
    fn into_header_value_marks_the_value_sensitive() {
        unsafe { env::set_var("A2M_TEST_SECRET_SENSITIVE", "x") };
        let secret = Secret::load("A2M_TEST_SECRET_SENSITIVE").expect("var is set");
        let header = secret.into_header_value("").expect("valid header");
        assert!(header.is_sensitive());
        unsafe { env::remove_var("A2M_TEST_SECRET_SENSITIVE") };
    }

    #[test]
    fn into_header_value_rejects_a_value_with_control_bytes() {
        unsafe { env::set_var("A2M_TEST_SECRET_INVALID", "line1\nline2") };
        let secret = Secret::load("A2M_TEST_SECRET_INVALID").expect("var is set");
        let err = secret.into_header_value("").unwrap_err();
        assert_eq!(err, CredError::InvalidHeaderValue);
        unsafe { env::remove_var("A2M_TEST_SECRET_INVALID") };
    }
}
