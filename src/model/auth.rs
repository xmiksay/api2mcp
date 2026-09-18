//! How an outgoing request to a service is authenticated.
//!
//! A credential reaches a request from one of two places, named by [`CredentialSource`]: an
//! environment variable ([`CredentialSource::Env`], the original and still the right choice for a
//! single shared credential), or the provider's own row ([`CredentialSource::Stored`]).
//!
//! **`Stored` exists because per-user credentials cannot live in a process environment.** Every
//! definition here is owned, and each owner needs their own token for the same upstream; a
//! process-wide env var cannot express that, and adding a user would mean editing the server's
//! environment and restarting it. The value is held in plaintext in `auth_providers`, which is a
//! deliberate, accepted trade: anyone with read access to the database, or to a backup of it, has
//! every stored credential.
//!
//! **I4 is unchanged by this.** I4 is "a credential has no path to a `String` that reaches a
//! model-visible surface", not "a credential is never persisted": a [`crate::secret::Secret`]
//! loaded from a column has exactly the same shape as one loaded from the environment — no
//! `Display`, no `Serialize`, no `Deref` — and the one route out is still
//! `Secret::into_header_value`. The read/write API never returns a stored value (it reports only
//! whether one is set), the control-plane MCP surface cannot see auth providers at all, and
//! `pack::export` emits no value, so a shared pack still carries the *shape* of a credential
//! binding and never the credential.

use uuid::Uuid;

use super::origin::Origin;
use super::slug::Slug;

/// Where this provider's credential value comes from. An enum rather than two optional fields
/// so "an env key *and* a stored value" and "neither" are unrepresentable rather than merely
/// disallowed — the same reasoning as [`super::param::ParamLocation::Body`]'s inline pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// The value lives in the process environment under this variable name. The name itself is
    /// not secret; only what it points at is.
    Env(String),
    /// The value lives on this provider's own row. `None` means the row exists but nobody has
    /// set a value yet — what a fresh UI create or a `pack::import` produces, and a send-time
    /// [`crate::secret::CredError::MissingStoredCredential`] until an owner sets one.
    Stored(Option<crate::secret::Secret>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKind {
    /// A static credential, resolved from `credential`, rendered into `value_template`, and
    /// written into the `header_name` header. `{token}` is replaced by the credential; a value
    /// with no placeholder is treated as a literal prefix, so `"Bearer {token}"` and `"Bearer "`
    /// render the same header.
    StaticHeader,
    /// A credential obtained and refreshed via OAuth2 client-credentials. Refreshed by a
    /// background task on a cached token, never inside a run — see plan note "OAuth token
    /// refresh happens outside the run".
    OAuth2ClientCredentials,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProvider {
    /// The user who created this provider — always the same owner as `service_slug`'s service
    /// (enforced at write time, `store::auth_provider::create`). See `model::Service::owner_id`.
    pub owner_id: Uuid,
    pub slug: Slug,
    pub service_slug: Slug,
    pub kind: AuthKind,
    /// Where the credential value comes from — an env var name, or the row itself.
    pub credential: CredentialSource,
    pub header_name: String,
    pub value_template: String,
    pub scopes: Vec<String>,
    pub token_url: Option<url::Url>,
    /// The one origin this provider's credential may be sent to. Human-set, never agent-set
    /// (I5) — `resolve/auth_bind.rs` asserts this matches the api_call's origin before a plan
    /// can bind the two together.
    pub bound_origin: Origin,
}
