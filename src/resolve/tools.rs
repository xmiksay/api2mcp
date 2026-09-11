//! Turns the selected api_calls/scripts into `tools/list` rows: one [`PlannedTool`] per
//! selected item, its `endpoint_aliases` rename applied, its `inputSchema` generated, and its
//! effective budget folded — plus the "one name, one owner" check the whole plan depends on.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Budgets, EndpointTarget, ScriptDef, Slug};
use crate::schema::input_schema;

use super::ResolveError;
use super::plan::{PlannedApiCall, PlannedTool, ToolTarget};

/// Builds one [`PlannedTool`] per selected api_call/script, applying `endpoint_aliases`
/// (rename, not add-a-second-name — "one `PlannedTool` per selected api_call and script")
/// and rejecting any resulting name collision, whether alias-vs-alias, alias-vs-slug, or
/// slug-vs-slug.
pub(super) fn build_tools(
    aliases: &BTreeMap<String, EndpointTarget>,
    endpoint_budgets: &Budgets,
    calls: &BTreeMap<Slug, PlannedApiCall>,
    scripts: &BTreeMap<Slug, ScriptDef>,
) -> Result<Vec<PlannedTool>, ResolveError> {
    let alias_by_target = reverse_aliases(aliases)?;
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    let mut tools = Vec::with_capacity(calls.len() + scripts.len());

    for (slug, planned) in calls {
        let name = alias_by_target
            .get(&target_key_api_call(slug))
            .cloned()
            .unwrap_or_else(|| slug.as_str().to_owned());
        claim_name(&mut seen_names, &name)?;
        tools.push(PlannedTool {
            name,
            input_schema: input_schema(&planned.api_call.params),
            target: ToolTarget::ApiCall(slug.clone()),
            budgets: *endpoint_budgets,
        });
    }

    for (slug, script) in scripts {
        let name = alias_by_target
            .get(&target_key_script(slug))
            .cloned()
            .unwrap_or_else(|| slug.as_str().to_owned());
        claim_name(&mut seen_names, &name)?;
        tools.push(PlannedTool {
            name,
            input_schema: input_schema(&script.params),
            target: ToolTarget::Script(slug.clone()),
            budgets: Budgets::fold(*endpoint_budgets, script.budgets),
        });
    }

    // Stable, alphabetical tool order (I7) — independent of the arbitrary iteration order
    // `calls`/`scripts` happened to be visited in above.
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tools)
}

fn claim_name(seen: &mut BTreeSet<String>, name: &str) -> Result<(), ResolveError> {
    if !seen.insert(name.to_owned()) {
        return Err(ResolveError::DuplicateToolName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn target_key_api_call(slug: &Slug) -> String {
    format!("api_call:{}", slug.as_str())
}

fn target_key_script(slug: &Slug) -> String {
    format!("script:{}", slug.as_str())
}

/// `endpoint_aliases` has no constraint against two different alias rows naming the same
/// target; if that ever happens there is no principled way to pick one, so it's a plan
/// failure rather than a last-writer-wins.
fn reverse_aliases(
    aliases: &BTreeMap<String, EndpointTarget>,
) -> Result<BTreeMap<String, String>, ResolveError> {
    let mut by_target: BTreeMap<String, String> = BTreeMap::new();
    for (alias, target) in aliases {
        let key = match target {
            EndpointTarget::ApiCall(slug) => target_key_api_call(slug),
            EndpointTarget::Script(slug) => target_key_script(slug),
        };
        if let Some(existing) = by_target.insert(key.clone(), alias.clone()) {
            return Err(ResolveError::AmbiguousAlias {
                target: key,
                aliases: vec![existing, alias.clone()],
            });
        }
    }
    Ok(by_target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_aliases_detects_two_names_for_one_target() {
        let target = EndpointTarget::ApiCall("call-a".parse().unwrap());
        let aliases = BTreeMap::from([
            ("first-name".to_owned(), target.clone()),
            ("second-name".to_owned(), target),
        ]);
        let err = reverse_aliases(&aliases).unwrap_err();
        assert!(matches!(err, ResolveError::AmbiguousAlias { .. }));
    }

    #[test]
    fn reverse_aliases_allows_distinct_targets() {
        let aliases = BTreeMap::from([
            (
                "a".to_owned(),
                EndpointTarget::ApiCall("call-a".parse().unwrap()),
            ),
            (
                "b".to_owned(),
                EndpointTarget::Script("script-b".parse().unwrap()),
            ),
        ]);
        assert_eq!(reverse_aliases(&aliases).unwrap().len(), 2);
    }

    #[test]
    fn claim_name_rejects_a_repeat() {
        let mut seen = BTreeSet::new();
        claim_name(&mut seen, "foo").unwrap();
        let err = claim_name(&mut seen, "foo").unwrap_err();
        assert!(matches!(err, ResolveError::DuplicateToolName { .. }));
    }
}
