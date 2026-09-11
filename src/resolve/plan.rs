//! [`EndpointPlan`] — the immutable, fully-typed output of [`super::build_plan`]. Everything
//! here is already validated: templates parsed, projections compiled, auth bound, origins
//! computed, budgets folded. An executor (`runtime`, a later chunk) only ever reads this
//! type; it never re-derives any of it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use serde_json_path::JsonPath;

use crate::http::UrlTemplate;
use crate::model::{Access, ApiCall, Budgets, Cardinality, Origin, ParamType, Service, Slug};

/// A projection field with its JSONPath already parsed — the resolve-time compile step
/// `model::Projection`'s own doc comment defers to this crate's `project`/`resolve` layer.
#[derive(Debug, Clone)]
pub struct CompiledProjectionField {
    pub name: String,
    pub path: JsonPath,
    pub cardinality: Cardinality,
    pub coerce: Option<ParamType>,
}

/// A compiled [`crate::model::Projection`]: one [`JsonPath`] per field, in declaration order
/// (I7) — the same order the projected output's fields are emitted in.
#[derive(Debug, Clone, Default)]
pub struct CompiledProjection {
    pub fields: Vec<CompiledProjectionField>,
}

/// A selected api_call, ready to bind and send: its own definition, the service it targets,
/// its path template already parsed (I3), its projection (if any) already compiled, and its
/// statically-derived origin (I2) — always `Origin::of(&service.base_url)`, since
/// `http::bind` never lets a param move the request off that origin (I3). Computed once,
/// here, rather than re-derived by both `origins::reachable_origins` and `auth_bind`.
#[derive(Debug, Clone)]
pub struct PlannedApiCall {
    pub api_call: ApiCall,
    pub service: Service,
    pub origin: Origin,
    pub url_template: UrlTemplate,
    pub projection: Option<CompiledProjection>,
}

/// What an MCP tool name resolves to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolTarget {
    ApiCall(Slug),
    Script(Slug),
}

/// One row of `tools/list`: the name the agent sees, its generated `inputSchema`, what it
/// dispatches to, and the budget ceiling that applies when it runs. For an api_call tool
/// that ceiling is just the endpoint's own [`EndpointPlan::budgets`] (an `ApiCall` carries no
/// independent budget opinion); for a script tool it is
/// `Budgets::fold(endpoint.budgets, script.budgets)` (I6) — narrower, never wider.
#[derive(Debug, Clone)]
pub struct PlannedTool {
    pub name: String,
    pub input_schema: Value,
    pub target: ToolTarget,
    pub budgets: Budgets,
}

/// The compiled, immutable plan for one endpoint. See the module doc and
/// [`super::build_plan`] for how this gets built; nothing downstream may re-derive any of
/// these fields — that is the whole point of doing it once, here.
#[derive(Debug, Clone)]
pub struct EndpointPlan {
    pub slug: Slug,
    pub write_ceiling: Access,
    pub instructions: Option<String>,

    /// One [`PlannedTool`] per selected api_call and script — see [`super::build_plan`] for
    /// tool-name resolution (alias-or-slug) and the uniqueness check.
    pub tools: Vec<PlannedTool>,

    /// The endpoint's own tag-expression selection of api_calls, keyed by their own slug —
    /// not by tool name. This is also the full set of api_calls reachable through any
    /// selected script, since [`Self::callable_by`] is defined as an intersection with this
    /// map's keys (I1) and can therefore never name an api_call outside it.
    pub calls: BTreeMap<Slug, PlannedApiCall>,

    /// The endpoint's selected scripts, keyed by their own slug.
    pub scripts: BTreeMap<Slug, crate::model::ScriptDef>,

    /// script slug -> (alias used in that script's `api()`/`api_many()` calls -> api_call
    /// slug). I1's data structure: the intersection of a script's declared calls
    /// (`ScriptDef::callable`) with [`Self::calls`]'s keys. `runtime::dispatch` (a later
    /// chunk) resolves every script-initiated call through exactly this map; a name absent
    /// from it can never reach HTTP, whether because the script never declared it or because
    /// this endpoint doesn't expose it.
    pub callable_by: BTreeMap<Slug, BTreeMap<String, Slug>>,

    /// The statically computed reachable-origin set (I2) — every origin any selected
    /// api_call's service can send a request to.
    pub origins: BTreeSet<Origin>,

    /// The endpoint's own folded budget ceiling (I6). A script's own budget opinion narrows
    /// this further per-tool; see [`PlannedTool::budgets`].
    pub budgets: Budgets,

    /// sha256 (hex) over the canonical JSON of every definition this plan was built from —
    /// see [`super::digest`]. Two plans with an identical digest are guaranteed to behave
    /// identically; a differing digest means *something* in the underlying definitions
    /// changed, without needing a diff to find out.
    pub digest: String,
}

impl EndpointPlan {
    /// The tool named `name`, if any — the lookup `server::mcp::tools_call` (a later chunk)
    /// needs before it can dispatch.
    pub fn tool(&self, name: &str) -> Option<&PlannedTool> {
        self.tools.iter().find(|t| t.name == name)
    }
}
