//! Database access. Every function here returns [`crate::model`] types — an
//! `entity::Model` must never escape this module, or the layers above it stop being
//! testable without Postgres.
//!
//! One façade per aggregate ([`ServiceStore`], [`AuthProviderStore`], ...), bundled by
//! [`Stores`] and constructed on demand from a [`DatabaseConnection`] — the same
//! on-demand-façade shape as `chess-base`'s `AppState::engines()`, with the one amendment
//! this crate's layering depends on: every public function here returns a [`crate::model`]
//! type or a plain scalar, never `entity::Model`.
//!
//! JSONB columns (`origin_allowlist`, `*.scopes`, `redirect_uris`, `projection`,
//! `pagination`, `budgets`, `default_headers`, `query_fixed`) are converted to/from their
//! typed `model::` shape here — that conversion, and reporting a malformed value as a typed
//! [`StoreError::Malformed`] rather than panicking, is this layer's job, not a caller's.

mod api_call;
mod api_call_params;
mod api_call_projection;
mod auth_provider;
mod endpoint;
mod meta;
mod oauth;
mod oauth_tokens;
mod run;
mod script;
mod service;
mod service_token;
mod session;
mod tag;
#[cfg(test)]
pub(crate) mod test_support;
mod user;

pub use api_call::{ApiCallStore, TaggedApiCall};
pub use auth_provider::AuthProviderStore;
pub use endpoint::EndpointStore;
pub use meta::MetaStore;
pub use oauth::{
    ConsentRequest, NewConsentRequest, NewOauthClient, NewOauthCode, OauthClient, OauthCode,
    OauthStore,
};
pub use oauth_tokens::{IssuedOauthToken, OauthToken, TokenGrant};
pub use run::{
    NewRun, NewRunCall, RunCall, RunCallerKind, RunFilter, RunStatus, RunStore, RunSummary,
    RunTargetKind,
};
pub use script::{ScriptStore, TaggedScript};
pub use service::ServiceStore;
pub use service_token::{MintedServiceToken, ServiceTokenRecord, ServiceTokenStore};
pub use session::{SessionStore, SessionUser};
pub use tag::TagStore;
pub use user::{NewUser, UserRecord, UserStore};

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;

use crate::model::{Access, Origin, Slug};

/// Every error a store can return. `DbErr` never reaches a caller directly (a SQL string
/// naming tables and constraints must not be one `?` away from a tool response) — every
/// site that can fail on the connection maps it through [`db_err`] first, which logs the
/// original error and returns [`StoreError::Db`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The requested row doesn't exist.
    #[error("not found")]
    NotFound,
    /// A row exists whose stored value can't be decoded into its `model::` shape — a
    /// malformed JSONB array, an unparseable slug/origin/URL, or a `location`/`kind` string
    /// outside the set the DB `CHECK` constraint should have enforced.
    #[error("malformed database row: {0}")]
    Malformed(String),
    /// A write would violate an application-level rule the DB schema doesn't itself encode
    /// (e.g. a duplicate slug surfaced as a friendlier message than the raw unique-index
    /// violation).
    #[error("{0}")]
    Conflict(String),
    /// Something went wrong talking to Postgres. The original [`DbErr`] is logged via
    /// `tracing::error!` at the call site and deliberately not carried here.
    #[error("database error")]
    Db,
    /// An internal invariant failed outside the database itself — a crypto operation
    /// (argon2 hashing/verification, sha256 token minting) or an unparsable stored value
    /// that isn't a plain JSONB/column decode issue.
    #[error("{0}")]
    Internal(String),
}

/// Maps a [`DbErr`] to [`StoreError::Db`], logging the original error (which can contain
/// table/constraint names) at `error` level first. Every `?` on a `sea_orm` call in this
/// module goes through this, never bare `From<DbErr>`.
pub(crate) fn db_err(context: &'static str) -> impl Fn(DbErr) -> StoreError {
    move |e| {
        tracing::error!(error = %e, context, "store: database operation failed");
        StoreError::Db
    }
}

pub(crate) fn parse_slug(s: &str) -> Result<Slug, StoreError> {
    Slug::from_str(s).map_err(|e| StoreError::Malformed(format!("invalid slug {s:?}: {e}")))
}

pub(crate) fn parse_url(s: &str) -> Result<url::Url, StoreError> {
    url::Url::parse(s).map_err(|e| StoreError::Malformed(format!("invalid URL {s:?}: {e}")))
}

pub(crate) fn parse_origin(s: &str) -> Result<Origin, StoreError> {
    Origin::from_str(s).map_err(|e| StoreError::Malformed(format!("invalid origin {s:?}: {e}")))
}

/// Decodes a JSONB array of strings (`origin_allowlist`, `scopes`, `redirect_uris`, ...)
/// into a `Vec<String>`, reporting any non-array or non-string element as
/// [`StoreError::Malformed`] rather than silently skipping it — a silently-dropped array
/// element is exactly the kind of malformed-row bug this layer exists to catch.
pub(crate) fn json_string_array(value: &Value, column: &str) -> Result<Vec<String>, StoreError> {
    let arr = value
        .as_array()
        .ok_or_else(|| StoreError::Malformed(format!("{column}: expected a JSON array")))?;
    arr.iter()
        .map(|v| {
            v.as_str().map(str::to_owned).ok_or_else(|| {
                StoreError::Malformed(format!("{column}: array element is not a string"))
            })
        })
        .collect()
}

/// As [`json_string_array`], but parsing every element as an [`Origin`] and collecting into
/// a `BTreeSet` — the shape `Service::origin_allowlist` needs (I7 bans `HashSet` here).
pub(crate) fn json_origin_set(value: &Value, column: &str) -> Result<BTreeSet<Origin>, StoreError> {
    json_string_array(value, column)?
        .into_iter()
        .map(|s| parse_origin(&s))
        .collect()
}

/// Decodes a JSONB object of string values (`default_headers`, `query_fixed`) into a
/// `BTreeMap<String, String>`.
pub(crate) fn json_string_map(
    value: &Value,
    column: &str,
) -> Result<BTreeMap<String, String>, StoreError> {
    let obj = value
        .as_object()
        .ok_or_else(|| StoreError::Malformed(format!("{column}: expected a JSON object")))?;
    obj.iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|s| (k.clone(), s.to_owned()))
                .ok_or_else(|| {
                    StoreError::Malformed(format!("{column}: value for {k:?} is not a string"))
                })
        })
        .collect()
}

/// `access` is `read`|`write` in three tables (`api_calls`, `endpoints.write_ceiling`) — one
/// conversion, shared, so the two spellings can never drift apart.
pub(crate) fn access_to_str(access: Access) -> &'static str {
    match access {
        Access::Read => "read",
        Access::Write => "write",
    }
}

pub(crate) fn str_to_access(s: &str) -> Result<Access, StoreError> {
    match s {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(StoreError::Malformed(format!(
            "expected 'read' or 'write', got {other:?}"
        ))),
    }
}

/// Lowercase hex sha256 of `bytes` — the fast-hash-at-rest scheme shared by
/// `service_tokens.token_hash`, `sessions.token_hash` and the oauth `*_hash` columns. A
/// token is 244+ bits of randomness, so a fast hash is safe here and a KDF on every MCP call
/// (as `user.rs` rightly uses for human passwords) is not.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `Param::enum_values` is `Option<Vec<Value>>`; the DB column is one nullable JSONB value
/// holding a JSON array. `None` (no enum constraint) and `Some(vec![])` (an enum with no
/// allowed values — a definer error the DB doesn't forbid, but not this layer's job to
/// reject) both round-trip: `None` -> SQL NULL, `Some(v)` -> a JSON array, always.
pub(crate) fn enum_values_to_json(values: &Option<Vec<Value>>) -> Option<Value> {
    values.as_ref().map(|v| Value::Array(v.clone()))
}

pub(crate) fn json_to_enum_values(
    value: Option<Value>,
    column: &str,
) -> Result<Option<Vec<Value>>, StoreError> {
    value
        .map(|v| match v {
            Value::Array(arr) => Ok(arr),
            _ => Err(StoreError::Malformed(format!(
                "{column}: expected a JSON array"
            ))),
        })
        .transpose()
}

pub(crate) fn strings_to_json(values: &[String]) -> Value {
    Value::Array(values.iter().map(|s| Value::String(s.clone())).collect())
}

pub(crate) fn origins_to_json(origins: &BTreeSet<Origin>) -> Value {
    Value::Array(
        origins
            .iter()
            .map(|o| Value::String(o.to_string()))
            .collect(),
    )
}

pub(crate) fn string_map_to_json(map: &BTreeMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

/// Bundles every aggregate's façade behind one on-demand handle, built from a live
/// connection the same way `AppState::stores()` will in `server/state.rs` (owned by
/// another chunk).
#[derive(Clone)]
pub struct Stores {
    db: DatabaseConnection,
}

impl Stores {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn service(&self) -> ServiceStore {
        ServiceStore::new(self.db.clone())
    }

    pub fn auth_provider(&self) -> AuthProviderStore {
        AuthProviderStore::new(self.db.clone())
    }

    pub fn api_call(&self) -> ApiCallStore {
        ApiCallStore::new(self.db.clone())
    }

    pub fn script(&self) -> ScriptStore {
        ScriptStore::new(self.db.clone())
    }

    pub fn tag(&self) -> TagStore {
        TagStore::new(self.db.clone())
    }

    pub fn endpoint(&self) -> EndpointStore {
        EndpointStore::new(self.db.clone())
    }

    pub fn run(&self) -> RunStore {
        RunStore::new(self.db.clone())
    }

    pub fn user(&self) -> UserStore {
        UserStore::new(self.db.clone())
    }

    pub fn service_token(&self) -> ServiceTokenStore {
        ServiceTokenStore::new(self.db.clone())
    }

    pub fn oauth(&self) -> OauthStore {
        OauthStore::new(self.db.clone())
    }

    pub fn meta(&self) -> MetaStore {
        MetaStore::new(self.db.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_string_array_rejects_non_array() {
        let err = json_string_array(&json!({"a": 1}), "col").unwrap_err();
        assert!(matches!(err, StoreError::Malformed(_)));
    }

    #[test]
    fn json_string_array_rejects_non_string_element() {
        let err = json_string_array(&json!(["a", 1]), "col").unwrap_err();
        assert!(matches!(err, StoreError::Malformed(_)));
    }

    #[test]
    fn json_origin_set_parses_valid_origins() {
        let set = json_origin_set(
            &json!(["https://a.example.com", "https://b.example.com"]),
            "col",
        )
        .unwrap();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn json_origin_set_rejects_malformed_origin() {
        let err = json_origin_set(&json!(["not a url"]), "col").unwrap_err();
        assert!(matches!(err, StoreError::Malformed(_)));
    }

    #[test]
    fn roundtrip_string_array_json() {
        let values = vec!["mcp".to_string(), "admin".to_string()];
        let json = strings_to_json(&values);
        let back = json_string_array(&json, "col").unwrap();
        assert_eq!(values, back);
    }
}
