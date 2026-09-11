//! How an outgoing request to a service is authenticated. Note what's *not* here: no credential
//! value. `credential_env_key` is an env var **name**; the value it points at is read at send
//! time into a [`crate::secret::Secret`] and never stored on this type. That's I4's structural
//! half — no column, and therefore no field here, can hold a credential value.

use super::origin::Origin;
use super::slug::Slug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKind {
    /// A static credential, read from `credential_env_key`, rendered into `value_template`, and
    /// written into the `header_name` header. `{token}` is replaced by the credential; a value
    /// with no placeholder is treated as a literal prefix, so `"Bearer {token}"` and `"Bearer "`
    /// render the same header. The credential itself is never stored here — only
    /// `credential_env_key`, the name of the environment variable holding it.
    StaticHeader,
    /// A credential obtained and refreshed via OAuth2 client-credentials. Refreshed by a
    /// background task on a cached token, never inside a run — see plan note "OAuth token
    /// refresh happens outside the run".
    OAuth2ClientCredentials,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProvider {
    pub slug: Slug,
    pub service_slug: Slug,
    pub kind: AuthKind,
    /// Name of the environment variable holding the credential value — never the value itself.
    pub credential_env_key: String,
    pub header_name: String,
    pub value_template: String,
    pub scopes: Vec<String>,
    pub token_url: Option<url::Url>,
    /// The one origin this provider's credential may be sent to. Human-set, never agent-set
    /// (I5) — `resolve/auth_bind.rs` asserts this matches the api_call's origin before a plan
    /// can bind the two together.
    pub bound_origin: Origin,
}
