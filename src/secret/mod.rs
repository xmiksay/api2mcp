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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use zeroize::Zeroizing;

/// A credential value, read either from the environment or from the row that stores it. See the
/// module docs for what's deliberately *not* implemented on this type. `Clone` is safe to derive:
/// it duplicates the wrapped `Zeroizing<String>` (each copy still zeroizes its own memory on
/// drop) without adding any new way to read the value out as a plain `String`.
///
/// `PartialEq`/`Eq` compare the wrapped values directly, and deliberately not in constant time:
/// they exist so [`crate::model::AuthProvider`] can stay comparable for definition diffing, and
/// both sides of any comparison here are already in this process's memory. This is not an
/// authentication primitive and must never be used as one.
#[derive(Clone, PartialEq, Eq)]
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
    #[error("auth provider stores its credential on the row, but no value has been set yet")]
    MissingStoredCredential,
}

impl Secret {
    /// Reads the credential value from the environment variable named `env_key`. The name
    /// itself is never a secret — it's `AuthProvider::credential_env_key`, a plain DB column —
    /// only the value it points at is.
    pub fn load(env_key: &str) -> Result<Secret, CredError> {
        let value = env::var(env_key).map_err(|_| CredError::MissingEnvVar {
            env_key: env_key.to_owned(),
        })?;
        Ok(Secret::from_raw(value))
    }

    /// Wraps an already-obtained value — one a caller resolved through its own testable
    /// env-lookup indirection (like `config::Config::from_lookup`), or one read back out of
    /// `auth_providers.credential_value` by [`crate::store`]. `pub(crate)`: the point is that
    /// everything *above* the store boundary still has no way to mint a `Secret` from arbitrary
    /// application data, and no way to read one back out again.
    pub(crate) fn from_raw(value: String) -> Secret {
        Secret(Zeroizing::new(value))
    }

    /// The single, deliberate read-back of a stored credential, used only by
    /// [`crate::store::auth_provider`] on its way into the `auth_providers.credential_value`
    /// column. `pub(crate)` and named to be greppable: this is the one place a credential
    /// becomes a `String` again, and it exists because the value has to reach the database.
    /// Nothing model-visible may call it — that is what keeps I4 true (see the module docs).
    pub(crate) fn expose_for_storage(&self) -> &str {
        self.0.as_str()
    }

    /// The one exit from this type: renders `template` into a sensitive `http::HeaderValue` with
    /// the credential substituted in.
    ///
    /// `{token}` is replaced by the credential wherever it appears; a template with no `{token}`
    /// is treated as a literal prefix, so both `"Bearer {token}"` and `"Bearer "` produce the
    /// same header. Both spellings exist in the wild here — the field's own doc and every test
    /// fixture used the placeholder form while the code treated it as a prefix, which would have
    /// rendered `Bearer {token}<credential>` for anyone who followed the documentation.
    ///
    /// The substitution happens *inside* this type and its result goes straight into a sensitive
    /// `HeaderValue`, so the credential never becomes a `String` a caller can hold (I4).
    pub(crate) fn into_header_value(
        self,
        template: &str,
    ) -> Result<::http::HeaderValue, CredError> {
        const PLACEHOLDER: &str = "{token}";
        let rendered = if template.contains(PLACEHOLDER) {
            template.replace(PLACEHOLDER, self.0.as_str())
        } else {
            format!("{template}{}", self.0.as_str())
        };
        let mut value =
            ::http::HeaderValue::from_str(&rendered).map_err(|_| CredError::InvalidHeaderValue)?;
        value.set_sensitive(true);
        Ok(value)
    }

    /// Renders `Basic base64(client_id:secret)` (RFC 6749 §2.3.1, `client_secret_basic`)
    /// directly into a sensitive `Authorization` header value — `server::oidc`'s token-exchange
    /// authentication. Chosen over `client_secret_post` (the secret as a form field) because a
    /// header value has an established sensitive-marking mechanism here
    /// ([`::http::HeaderValue::set_sensitive`]); a request body does not. `client_id` is not
    /// itself secret, only ever this method's other half.
    pub(crate) fn into_basic_auth_header(
        self,
        client_id: &str,
    ) -> Result<::http::HeaderValue, CredError> {
        let raw = format!("{client_id}:{}", self.0.as_str());
        let encoded = BASE64_STANDARD.encode(raw.as_bytes());
        let mut value = ::http::HeaderValue::from_str(&format!("Basic {encoded}"))
            .map_err(|_| CredError::InvalidHeaderValue)?;
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
    fn a_token_placeholder_is_substituted_not_appended() {
        let secret = Secret::from_raw("s3cr3t".to_owned());
        let header = secret
            .into_header_value("Bearer {token}")
            .expect("valid header");
        assert_eq!(header.to_str().expect("ascii"), "Bearer s3cr3t");
    }

    #[test]
    fn a_template_without_a_placeholder_is_a_prefix() {
        let secret = Secret::from_raw("s3cr3t".to_owned());
        let header = secret.into_header_value("Bearer ").expect("valid header");
        assert_eq!(header.to_str().expect("ascii"), "Bearer s3cr3t");
    }

    #[test]
    fn a_placeholder_can_sit_mid_template() {
        let secret = Secret::from_raw("k".to_owned());
        let header = secret
            .into_header_value("token={token}; v=1")
            .expect("valid header");
        assert_eq!(header.to_str().expect("ascii"), "token=k; v=1");
    }

    #[test]
    fn from_raw_roundtrips_like_load() {
        let secret = Secret::from_raw("raw-value".to_owned());
        let header = secret.into_header_value("").expect("valid header");
        assert_eq!(header.to_str().expect("ascii"), "raw-value");
    }

    #[test]
    fn basic_auth_header_encodes_client_id_and_secret_and_is_sensitive() {
        let secret = Secret::from_raw("s3cr3t".to_owned());
        let header = secret
            .into_basic_auth_header("client-1")
            .expect("valid header");
        assert!(header.is_sensitive());
        let expected = format!("Basic {}", BASE64_STANDARD.encode(b"client-1:s3cr3t"));
        assert_eq!(header.to_str().expect("ascii"), expected);
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
