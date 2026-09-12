//! An endpoint is an MCP surface: a tag expression selecting which api_calls/scripts are exposed
//! as tools, plus a write ceiling and a folded budget. `resolve::build_plan` is what actually
//! evaluates `tag_expr` against every candidate's tags and enforces `write_ceiling` — this type
//! only holds the compiled-from-row shape.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use uuid::Uuid;

use super::api_call::Access;
use super::budget::Budgets;
use super::slug::Slug;
use super::tag::TagExpr;

/// One `endpoint_aliases` row: an alternate tool name for an api_call or script selected by
/// `tag_expr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointTarget {
    ApiCall(Slug),
    Script(Slug),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndpointDef {
    /// The user who created this endpoint. See `model::Service::owner_id`. Every alias target
    /// and every entry in `auth_providers` must belong to this same owner (enforced at write
    /// time, `store::endpoint`).
    pub owner_id: Uuid,
    pub slug: Slug,
    pub tag_expr: TagExpr,
    /// The highest `Access` any api_call/script selected by this endpoint may declare.
    pub write_ceiling: Access,
    pub budgets: Budgets,
    pub instructions: Option<String>,
    pub enabled: bool,
    /// alias -> target, from `endpoint_aliases`.
    pub aliases: BTreeMap<String, EndpointTarget>,
    /// `endpoint_auth_providers`: which auth providers this endpoint may bind to, on top of
    /// whatever an api_call already declares.
    pub auth_providers: BTreeSet<Slug>,
}
