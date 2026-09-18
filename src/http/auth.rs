//! Applies an [`AuthProvider`] to an outgoing request. The credential's only route into the
//! request is [`Secret::into_header_value`] — nothing in this module ever turns a credential
//! into a `String` it could format, log, or return.
//!
//! `value_template` accepts both spellings that exist in this codebase: `"Bearer {token}"`
//! substitutes the credential for the placeholder, and `"Bearer "` appends it. They render
//! identically, which matters because the field's own documentation and every test fixture used
//! the placeholder form while the code treated it as a prefix — following the docs would have
//! produced the header `Bearer {token}<credential>`. The substitution happens inside [`Secret`],
//! so the credential never becomes a `String` this module could hold (I4).

use serde::Serialize;
use thiserror::Error;

use crate::model::{AuthProvider, CredentialSource, Origin};
use crate::secret::{CredError, Secret};

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum AuthError {
    #[error("credential: {0}")]
    Credential(#[from] CredError),
    #[error("header name {name:?} configured on the auth provider is not a valid header name")]
    InvalidHeaderName { name: String },
}

/// Attaches `provider`'s credential to `headers` for a request going to `url` — but only when
/// `url`'s origin matches `provider.bound_origin` (I5). A mismatch is not an error: it is exactly
/// the case a cross-origin redirect hop produces, and the whole point is that such a hop must
/// come away with **no** credential attached, silently, rather than carrying the wrong one.
pub fn apply(
    provider: &AuthProvider,
    url: &url::Url,
    headers: &mut ::http::HeaderMap,
) -> Result<(), AuthError> {
    let Ok(origin) = Origin::of(url) else {
        return Ok(());
    };
    if origin != provider.bound_origin {
        return Ok(());
    }

    let secret = match &provider.credential {
        CredentialSource::Env(key) => Secret::load(key)?,
        CredentialSource::Stored(Some(value)) => value.clone(),
        CredentialSource::Stored(None) => return Err(CredError::MissingStoredCredential.into()),
    };
    let value = secret.into_header_value(&provider.value_template)?;
    let name = ::http::HeaderName::try_from(provider.header_name.as_str()).map_err(|_| {
        AuthError::InvalidHeaderName {
            name: provider.header_name.clone(),
        }
    })?;
    headers.insert(name, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthKind, Slug};
    use std::str::FromStr;

    // Env vars are process-global, shared across every concurrently-running test thread — each
    // test below claims its own key (mirroring `secret::tests`'s convention) rather than reusing
    // one, since a shared name would make "expects the var to be absent" flaky against whichever
    // other test happened to have it set at the same moment.
    fn provider(bound_origin: &str, credential_env_key: &str) -> AuthProvider {
        with_credential(
            bound_origin,
            CredentialSource::Env(credential_env_key.to_owned()),
        )
    }

    fn with_credential(bound_origin: &str, credential: CredentialSource) -> AuthProvider {
        AuthProvider {
            owner_id: uuid::Uuid::nil(),
            slug: Slug::from_str("demo-auth").expect("valid slug"),
            service_slug: Slug::from_str("demo").expect("valid slug"),
            kind: AuthKind::StaticHeader,
            credential,
            header_name: "Authorization".to_owned(),
            value_template: "Bearer ".to_owned(),
            scopes: Vec::new(),
            token_url: None,
            bound_origin: bound_origin.parse().expect("valid origin"),
        }
    }

    #[test]
    fn attaches_the_header_when_the_origin_matches() {
        unsafe { std::env::set_var("A2M_TEST_AUTH_APPLY_MATCH", "sh-token") };
        let mut headers = ::http::HeaderMap::new();
        let url = url::Url::parse("https://api.example.com/x").expect("valid url");
        apply(
            &provider("https://api.example.com", "A2M_TEST_AUTH_APPLY_MATCH"),
            &url,
            &mut headers,
        )
        .expect("applies");
        let value = headers
            .get(::http::header::AUTHORIZATION)
            .expect("header set");
        assert_eq!(value.to_str().expect("ascii"), "Bearer sh-token");
        assert!(value.is_sensitive());
        unsafe { std::env::remove_var("A2M_TEST_AUTH_APPLY_MATCH") };
    }

    #[test]
    fn skips_silently_on_a_cross_origin_url() {
        unsafe { std::env::set_var("A2M_TEST_AUTH_APPLY_CROSS_ORIGIN", "sh-token") };
        let mut headers = ::http::HeaderMap::new();
        let url = url::Url::parse("https://evil.example.com/x").expect("valid url");
        apply(
            &provider(
                "https://api.example.com",
                "A2M_TEST_AUTH_APPLY_CROSS_ORIGIN",
            ),
            &url,
            &mut headers,
        )
        .expect("no error");
        assert!(headers.get(::http::header::AUTHORIZATION).is_none());
        unsafe { std::env::remove_var("A2M_TEST_AUTH_APPLY_CROSS_ORIGIN") };
    }

    #[test]
    fn missing_credential_env_var_is_an_error() {
        let mut headers = ::http::HeaderMap::new();
        let url = url::Url::parse("https://api.example.com/x").expect("valid url");
        let err = apply(
            &provider(
                "https://api.example.com",
                "A2M_TEST_AUTH_APPLY_DEFINITELY_UNSET",
            ),
            &url,
            &mut headers,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AuthError::Credential(CredError::MissingEnvVar { .. })
        ));
    }

    #[test]
    fn a_stored_credential_needs_no_environment_variable() {
        let mut headers = ::http::HeaderMap::new();
        let url = url::Url::parse("https://api.example.com/x").expect("valid url");
        apply(
            &with_credential(
                "https://api.example.com",
                CredentialSource::Stored(Some(Secret::from_raw("sh-stored".to_owned()))),
            ),
            &url,
            &mut headers,
        )
        .expect("applies");
        let value = headers
            .get(::http::header::AUTHORIZATION)
            .expect("header set");
        assert_eq!(value.to_str().expect("ascii"), "Bearer sh-stored");
        assert!(
            value.is_sensitive(),
            "a stored credential is still sensitive"
        );
    }

    /// A stored-source provider whose owner has not set a value yet must fail loudly at send
    /// time, never send the request unauthenticated and let a 401 look like the upstream's fault.
    #[test]
    fn an_unset_stored_credential_is_an_error() {
        let mut headers = ::http::HeaderMap::new();
        let url = url::Url::parse("https://api.example.com/x").expect("valid url");
        let err = apply(
            &with_credential("https://api.example.com", CredentialSource::Stored(None)),
            &url,
            &mut headers,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AuthError::Credential(CredError::MissingStoredCredential)
        ));
        assert!(headers.get(::http::header::AUTHORIZATION).is_none());
    }
}
