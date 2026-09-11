//! A Rhai script exposed as a tool. `callable` is I1's declarative half: the compiled form of the
//! `script_api_calls` join, mapping the alias a script uses in `api("alias", ...)` to the
//! api_call it actually names. `runtime::dispatch` resolves through exactly this map (intersected
//! with what the endpoint exposes) — a name not in it can never reach HTTP.

use std::collections::BTreeMap;

use super::budget::Budgets;
use super::param::Param;
use super::slug::Slug;

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptDef {
    pub slug: Slug,
    /// Rhai source text; compiling it into an `AST` happens in `script::engine`, not here.
    pub source: String,
    pub params: Vec<Param>,
    /// alias (as used inside the script's `api()`/`api_many()` calls) -> declared api_call.
    pub callable: BTreeMap<String, Slug>,
    /// A script's own budget opinion. Never used standalone — always folded with its endpoint's
    /// via `Budgets::fold` (I6), so it can only narrow, never widen, what the endpoint allows.
    pub budgets: Budgets,
    pub description: Option<String>,
}
