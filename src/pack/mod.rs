//! YAML packs: the portable, credential-free export/import format for a curated set of
//! services, api_calls, scripts and endpoints — the seed for `plan.md` §6 ("Sdílení"). Postgres
//! is this crate's source of truth (see `lib.rs`'s module doc); a pack is what a human moves
//! between instances, or checks into git as `examples/demo.pack.yaml`.
//!
//! Three properties are non-negotiable (`plan.md` §6 restated in the C13 brief):
//! - **A pack never contains a credential, nor a reference to one.** There is no field anywhere
//!   in this module a secret *value*, or even an auth-provider *reference*, could occupy — a
//!   pack carries no auth providers at all (see this module's own note below).
//!   [`validate_credentials`] is the backstop for the one way that guarantee could still be
//!   defeated: a human pasting a live token into a free-text field that wasn't meant to hold one.
//! - **A pack never contains database ids or timestamps.** Every reference here is a slug
//!   (`String` on the wire, validated into [`crate::model::Slug`] by [`convert`]), so importing
//!   the same pack into a different instance reproduces the identical
//!   [`crate::resolve::EndpointPlan`] digest — that round trip is `tests/pack_roundtrip.rs`'s
//!   highest-value assertion.
//! - **A pack contains no executable code beyond pure transformations.** A projection is a
//!   declarative JSONPath string. A script's Rhai source is the one deliberate exception, and is
//!   exactly why plan.md's import review treats tool descriptions (and, by extension, a script's
//!   source) as untrusted content before it reaches a human's screen.
//!
//! This module only defines the document shape and its conversions to/from [`crate::model`]
//! types ([`convert`], private — [`export`]/[`import`]/[`validate`] are its only callers).
//! [`export::export_endpoint`] walks a live database into a [`Pack`]; [`validate::validate`]
//! checks a `Pack` value in isolation, before any transaction opens; [`import::import`] upserts
//! a validated `Pack` into the database.
//!
//! **A pack carries no auth providers at all** — not the definitions (there is no
//! `auth_providers` map on [`Pack`]) and not even a reference to one from an api_call (an
//! api_call names no provider of its own in the first place — see `model::ApiCall`'s doc: a
//! service has at most one, and every api_call on it uses it). `#[serde(deny_unknown_fields)]`
//! on [`Pack`] turns an older pack's top-level `auth_providers:` key into a parse error rather
//! than a silently-ignored, half-imported one. `PackEndpoint::auth_providers` (the endpoint's
//! own provider *scope*, a set of bare slugs) is the one survivor — it doubles as the
//! read-write admin API's live DTO field (`server::api::dto`), so it stays even though
//! [`export::export_endpoint`] always emits it empty; a hand-written pack that sets it is
//! importable only if the target instance already has matching provider slugs.

mod convert;
pub mod export;
pub mod import;
pub mod validate;
// `pub(crate)`, not private: `server::api::auth_providers` reuses `looks_like_credential`
// directly for its own live pre-write check, now that a pack carries no auth providers for
// `pack::validate` to run that heuristic over on its behalf (see this module's own doc).
pub(crate) mod validate_credentials;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use convert::ConvertError;
pub use export::{ExportError, export_endpoint};
pub use import::{ImportChange, ImportError, ImportReport, import};
pub use validate::{ValidationError, validate};

/// The only format version this crate writes or accepts. Bumped only on a breaking shape
/// change; a mismatch is a [`ValidationError`], not a raw serde failure, so the error names the
/// pack's own declared version instead of an opaque "missing field" message.
pub const PACK_VERSION: u32 = 1;

/// The document. Every collection is a `BTreeMap`/`BTreeSet` — never `Hash*` — so re-exporting
/// the same definitions always produces byte-identical YAML (I7's "no `Hash*`" rule, extended to
/// this crate's one on-disk format: a diff between two exports of an unchanged definition set
/// must be empty).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub version: u32,
    #[serde(default)]
    pub services: BTreeMap<String, PackService>,
    #[serde(default)]
    pub api_calls: BTreeMap<String, PackApiCall>,
    #[serde(default)]
    pub scripts: BTreeMap<String, PackScript>,
    #[serde(default)]
    pub endpoints: BTreeMap<String, PackEndpoint>,
    /// The full tag vocabulary used by [`Self::api_calls`] and [`Self::scripts`] in this pack —
    /// informational (a human skimming the YAML sees the vocabulary in one place without
    /// hunting through every item) and checked by [`validate`] to actually equal that union, so
    /// it can never silently drift from what the items themselves declare.
    #[serde(default)]
    pub tags: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackService {
    pub base_url: String,
    #[serde(default)]
    pub origin_allowlist: BTreeSet<String>,
    #[serde(default)]
    pub default_headers: BTreeMap<String, String>,
    pub timeout_ms: u32,
    pub max_concurrency: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_per_min: Option<u32>,
    pub max_response_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackAuthKind {
    StaticHeader,
    OAuth2ClientCredentials,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackAuthProvider {
    /// The service slug this provider authenticates against.
    pub service: String,
    pub kind: PackAuthKind,
    /// An environment variable *name* — never a value. Absent means the provider's credential
    /// is stored on its own row instead, and a pack deliberately carries no way to express what
    /// that value is: importing such a provider creates it with no value set, for the importing
    /// owner to fill in themselves. See this module's doc for why no sibling field here could
    /// ever hold a value instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env_key: Option<String>,
    pub header_name: String,
    pub value_template: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    pub bound_origin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackParamLocation {
    Path,
    Query,
    Header,
    /// The JSON pointer (as a plain string; see [`convert`] for the parse) the value is spliced
    /// into the request body at.
    Body(String),
    /// Never binds into an HTTP request — a script's own input. See
    /// [`crate::model::ParamLocation::Local`].
    Local,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackParam {
    pub name: String,
    pub location: PackParamLocation,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Set by the definer; a caller can never supply or see this. `validate` rejects
    /// `required: true` alongside a `fixed` value — a fixed param cannot also be required,
    /// mirroring the DB's own `CHECK(fixed_value IS NULL OR required = false)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub position: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackCardinality {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackProjectionField {
    pub name: String,
    pub path: String,
    pub cardinality: PackCardinality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coerce: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackProjection {
    #[serde(default)]
    pub fields: Vec<PackProjectionField>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackPagination {
    #[default]
    None,
    Cursor {
        next_cursor_path: String,
        query_param: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackBudgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackApiCall {
    /// Which service this call targets — and, transitively, which auth provider (if any) it
    /// uses: a service has at most one, and every api_call on it uses it. An api_call names no
    /// provider of its own (see `model::ApiCall`'s doc).
    pub service: String,
    /// An HTTP method token (e.g. `"GET"`), parsed by [`convert`].
    pub method: String,
    pub path_template: String,
    #[serde(default)]
    pub query_fixed: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_template: Option<Value>,
    /// `"read"` or `"write"`.
    pub access: String,
    #[serde(default)]
    pub idempotent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<PackProjection>,
    #[serde(default)]
    pub pagination: PackPagination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    #[serde(default)]
    pub params: Vec<PackParam>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// What a model reads to decide whether and when to call this tool. See this module's
    /// doc and `server::mcp::registry::describe` for why this is worth more than the
    /// method/path fallback synthesized when it's absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackScript {
    pub source: String,
    #[serde(default)]
    pub params: Vec<PackParam>,
    /// alias (as used inside the script's `api()`/`api_many()` calls) -> api_call slug.
    #[serde(default)]
    pub callable: BTreeMap<String, String>,
    #[serde(default)]
    pub budgets: PackBudgets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackEndpointTarget {
    ApiCall(String),
    Script(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackEndpoint {
    pub tag_expr: String,
    #[serde(default = "default_write_ceiling")]
    pub write_ceiling: String,
    #[serde(default)]
    pub budgets: PackBudgets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// alias -> target: renames a tool's exposed name away from its own slug.
    #[serde(default)]
    pub aliases: BTreeMap<String, PackEndpointTarget>,
    /// Which auth providers this endpoint may bind to — a service has at most one, and an
    /// api_call declares none of its own, so this restricts by service, not by api_call. Empty
    /// means "every provider belonging to a selected service" — see
    /// [`crate::resolve::auth_bind`]. Not carried by [`export::export_endpoint`] (a pack
    /// carries no auth providers at all — see this module's own doc); it survives here only
    /// because [`PackEndpoint`] doubles as the read-write admin API's live DTO
    /// (`server::api::dto::EndpointView`/`EndpointCreate`), where this scope is a real,
    /// still-live feature.
    #[serde(default)]
    pub auth_providers: BTreeSet<String>,
}

fn default_write_ceiling() -> String {
    "read".to_owned()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_pack() -> Pack {
        Pack {
            version: PACK_VERSION,
            services: BTreeMap::from([(
                "svc".to_owned(),
                PackService {
                    base_url: "https://svc.example.com/".to_owned(),
                    origin_allowlist: BTreeSet::from(["https://svc.example.com".to_owned()]),
                    default_headers: BTreeMap::new(),
                    timeout_ms: 5_000,
                    max_concurrency: 4,
                    rate_limit_per_min: None,
                    max_response_bytes: 1_000_000,
                },
            )]),
            api_calls: BTreeMap::new(),
            scripts: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            tags: BTreeSet::new(),
        }
    }

    #[test]
    fn yaml_round_trip_is_lossless() {
        let pack = minimal_pack();
        let yaml = serde_norway::to_string(&pack).expect("serializes");
        let back: Pack = serde_norway::from_str(&yaml).expect("deserializes");
        assert_eq!(pack, back);
    }

    #[test]
    fn missing_optional_fields_default_sensibly() {
        let yaml = "version: 1\nservices: {}\n";
        let pack: Pack = serde_norway::from_str(yaml).expect("deserializes");
        assert_eq!(pack.version, 1);
        assert!(pack.api_calls.is_empty());
        assert!(pack.tags.is_empty());
    }

    /// The specific regression Change 2 exists to prevent: an older pack's top-level
    /// `auth_providers:` key must fail to parse, not silently vanish into a half-imported pack.
    #[test]
    fn a_top_level_auth_providers_key_is_rejected_outright() {
        let yaml = "version: 1\nservices: {}\nauth_providers: {}\n";
        let err = serde_norway::from_str::<Pack>(yaml).unwrap_err();
        assert!(err.to_string().contains("auth_providers"));
    }
}
