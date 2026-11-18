//! The one import hazard [`super::validate`] cannot see, because it needs the database: a pack
//! retagging an api_call so that an endpoint *outside the pack* now selects it through
//! `internal_tag_expr` while still aliasing it. That endpoint's plan would then fail with
//! `ResolveError::AliasOfInternalCall`, and since every admin API write revalidates the owner's
//! whole definition set, the owner could be left with no single write that repairs it.

use crate::model::{EndpointDef, EndpointTarget, eval_tag_expr};

use super::Pack;
use super::convert::{self, ConvertError};

/// One human-readable line per alias in `stored` that `pack`'s incoming tags would make illegal.
/// An endpoint the pack itself carries is skipped: the pack replaces it, and
/// [`super::validate`] already checked the replacement.
pub(super) fn stored_alias_conflicts(
    stored: &[EndpointDef],
    pack: &Pack,
) -> Result<Vec<String>, ConvertError> {
    let mut conflicts = Vec::new();
    for endpoint in stored {
        let Some(internal) = &endpoint.internal_tag_expr else {
            continue;
        };
        if pack.endpoints.contains_key(endpoint.slug.as_str()) {
            continue;
        }
        for (alias, target) in &endpoint.aliases {
            let EndpointTarget::ApiCall(slug) = target else {
                continue;
            };
            let Some(call) = pack.api_calls.get(slug.as_str()) else {
                continue;
            };
            if eval_tag_expr(internal, &convert::tags_from_pack(&call.tags)?) {
                conflicts.push(format!(
                    "endpoint {:?}: alias {alias:?} names api_call {:?}, which this pack's tags \
                     would make internal to that endpoint",
                    endpoint.slug.as_str(),
                    slug.as_str(),
                ));
            }
        }
    }
    Ok(conflicts)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::model::{Access, Budgets, Tag, TagExpr};

    fn has(tag: &str) -> TagExpr {
        TagExpr::Has(Tag(tag.parse().unwrap()))
    }

    /// Stored endpoint `ep` with `internal_tag_expr: has(raw)` and alias `x` -> `call-c`.
    fn stored() -> EndpointDef {
        EndpointDef {
            owner_id: uuid::Uuid::nil(),
            slug: "ep".parse().unwrap(),
            tag_expr: has("public"),
            internal_tag_expr: Some(has("raw")),
            write_ceiling: Access::Read,
            budgets: Budgets::default(),
            instructions: None,
            enabled: true,
            aliases: BTreeMap::from([(
                "x".to_owned(),
                EndpointTarget::ApiCall("call-c".parse().unwrap()),
            )]),
            auth_providers: BTreeSet::new(),
        }
    }

    fn pack(tag: &str, extra: &str) -> Pack {
        serde_norway::from_str(&format!(
            "version: 1\n\
             api_calls:\n  \
               call-c:\n    \
                 service: svc\n    \
                 method: GET\n    \
                 path_template: /c\n    \
                 access: read\n    \
                 tags: [{tag}]\n\
             {extra}"
        ))
        .unwrap()
    }

    #[test]
    fn a_retag_into_the_internal_selection_is_a_conflict() {
        let conflicts = stored_alias_conflicts(&[stored()], &pack("raw", "")).unwrap();
        assert_eq!(conflicts.len(), 1);
        for name in ["\"ep\"", "\"x\"", "\"call-c\""] {
            assert!(conflicts[0].contains(name), "{}", conflicts[0]);
        }
    }

    #[test]
    fn a_tag_outside_the_internal_selection_is_fine() {
        assert!(
            stored_alias_conflicts(&[stored()], &pack("public", ""))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_endpoint_the_pack_replaces_is_skipped() {
        let replaced = pack("raw", "endpoints:\n  ep:\n    tag_expr: has(public)\n");
        assert!(
            stored_alias_conflicts(&[stored()], &replaced)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_endpoint_without_an_internal_selection_is_skipped() {
        let plain = EndpointDef {
            internal_tag_expr: None,
            ..stored()
        };
        assert!(
            stored_alias_conflicts(&[plain], &pack("raw", ""))
                .unwrap()
                .is_empty()
        );
    }
}
