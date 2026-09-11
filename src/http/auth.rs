//! Applies an [`AuthProvider`] to an outgoing request. The credential's only route into the
//! request is [`Secret::into_header_value`] — nothing in this module ever turns a credential
//! into a `String` it could format, log, or return.
//!
//! `value_template` is used exactly as `Secret::into_header_value`'s `prefix` argument (e.g.
//! `"Bearer "`), with the credential value appended after it by `Secret` itself. That is a
//! narrower reading than "template with a `{token}` placeholder" — see this crate's chunk report
//! for why: `Secret` only exposes a prefix-based renderer by design (I4), so a provider's
//! configured value must always take the form `"<literal prefix><credential>"`.

use serde::Serialize;
use thiserror::Error;

use crate::model::{AuthProvider, Origin};
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

    let secret = Secret::load(&provider.credential_env_key)?;
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
        AuthProvider {
            slug: Slug::from_str("demo-auth").expect("valid slug"),
            service_slug: Slug::from_str("demo").expect("valid slug"),
            kind: AuthKind::StaticHeader,
            credential_env_key: credential_env_key.to_owned(),
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
}
