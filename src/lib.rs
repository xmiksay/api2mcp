//! api2mcp — MCP Tool Factory.
//!
//! Turns curated HTTP API calls into MCP tools. A human defines a [`model::Service`], an
//! [`model::AuthProvider`], [`model::ApiCall`]s and [`model::ScriptDef`]s; an
//! [`model::EndpointDef`] selects a subset of them by tag expression; an agent connects over
//! MCP and sees only that subset.
//!
//! # The shape of the crate
//!
//! There is a compile step between "rows in Postgres" and "execution":
//! [`resolve::EndpointPlan`]. Rows (or an imported YAML pack) become a validated, immutable
//! plan, and only then can an executor exist. Every statically checkable invariant is checked
//! *once*, in [`resolve`], before anything can send a request. The executor re-checks the cheap
//! ones at send time as defence in depth, but it never *computes* them.
//!
//! The consequence worth protecting: `sea_orm` is a dependency of exactly three modules —
//! [`entity`], [`migration`] and [`store`]. Stores return [`model`] types, never `entity::Model`.
//! Everything above them is pure over hand-built structs and unit-testable with no database.
//!
//! # Invariants
//!
//! | # | Invariant | Enforced in |
//! |---|---|---|
//! | I1 | A script never performs HTTP. It can reach only the api_calls it declared, intersected with what its endpoint exposes. | [`runtime::dispatch`] |
//! | I2 | The set of origins an endpoint can reach is computable statically, before execution. | [`resolve::origins`] |
//! | I3 | A param value is always a leaf: it can never alter the request's origin, path structure, method, or any header outside the parameterisable-header allowlist. | [`http::url_template`], [`http::bind`] |
//! | I4 | A credential has no path to a `String` that reaches a model-visible surface. | [`secret::Secret`] (no `Display`/`Serialize`), [`http::redact`] |
//! | I5 | The auth-provider-to-origin binding is set by a human. An agent can neither propose nor change it. | [`resolve::auth_bind`], `pub(crate)` writes in [`store`] |
//! | I6 | Budgets (calls, bytes, wall clock, pages) are enforced by the runtime, never by the script. | [`runtime::budget`] |
//! | I7 | Execution is deterministic *given the same upstream responses*: no clock, no randomness, stable iteration order, fan-out results in input order. | [`runtime::fanout`], [`script::engine`] |
//!
//! `plan.md`'s I8 (versioned, immutable definitions) is deliberately **not** implemented.
//! Definitions are mutable and last-write-wins; the history lives in the run log, where each
//! run stores a complete snapshot of the definition that actually executed.

pub mod cli;
pub mod config;
pub mod db;
pub mod entity;
pub mod http;
pub mod migration;
pub mod model;
pub mod observe;
pub mod pack;
pub mod project;
pub mod resolve;
pub mod runtime;
pub mod schema;
pub mod script;
pub mod secret;
pub mod server;
pub mod store;
pub mod version;
