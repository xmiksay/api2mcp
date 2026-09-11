//! Tags select which api_calls/scripts an endpoint exposes. The boolean AST here is the
//! compiled form of an endpoint's `tag_expr` column; parsing the human-written expression string
//! into this AST is chunk C6's job ([`crate::resolve`]) — this module only defines the shape and
//! how to evaluate it.

use std::collections::BTreeSet;

use super::slug::Slug;

/// A tag is just a validated name; wrapping [`Slug`] rather than re-using it bare keeps
/// `TagExpr::Has(Tag)` from being confused with a `Slug` referring to a service or api_call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag(pub Slug);

/// Boolean expression over an item's tag set, e.g. `has(read) and not has(deprecated)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagExpr {
    Has(Tag),
    Not(Box<TagExpr>),
    And(Box<TagExpr>, Box<TagExpr>),
    Or(Box<TagExpr>, Box<TagExpr>),
}

/// Whether `tags` satisfies `expr`. A `BTreeSet` (never `HashSet`) so the same `(expr, tags)`
/// pair is evaluated identically regardless of insertion order (I7) — though evaluation order
/// doesn't actually change the boolean result here, using `HashSet` anywhere in this module
/// would be an easy invariant regression to introduce later by copy-paste.
pub fn eval(expr: &TagExpr, tags: &BTreeSet<Tag>) -> bool {
    match expr {
        TagExpr::Has(tag) => tags.contains(tag),
        TagExpr::Not(inner) => !eval(inner, tags),
        TagExpr::And(a, b) => eval(a, tags) && eval(b, tags),
        TagExpr::Or(a, b) => eval(a, tags) || eval(b, tags),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> Tag {
        Tag(s.parse().expect("valid slug"))
    }

    fn tags(names: &[&str]) -> BTreeSet<Tag> {
        names.iter().map(|n| tag(n)).collect()
    }

    #[test]
    fn has_matches_membership() {
        let expr = TagExpr::Has(tag("read"));
        assert!(eval(&expr, &tags(&["read", "write"])));
        assert!(!eval(&expr, &tags(&["write"])));
    }

    #[test]
    fn not_inverts() {
        let expr = TagExpr::Not(Box::new(TagExpr::Has(tag("deprecated"))));
        assert!(eval(&expr, &tags(&["read"])));
        assert!(!eval(&expr, &tags(&["read", "deprecated"])));
    }

    #[test]
    fn and_requires_both() {
        let expr = TagExpr::And(
            Box::new(TagExpr::Has(tag("read"))),
            Box::new(TagExpr::Has(tag("stable"))),
        );
        assert!(eval(&expr, &tags(&["read", "stable"])));
        assert!(!eval(&expr, &tags(&["read"])));
        assert!(!eval(&expr, &tags(&["stable"])));
    }

    #[test]
    fn or_requires_either() {
        let expr = TagExpr::Or(
            Box::new(TagExpr::Has(tag("read"))),
            Box::new(TagExpr::Has(tag("write"))),
        );
        assert!(eval(&expr, &tags(&["read"])));
        assert!(eval(&expr, &tags(&["write"])));
        assert!(!eval(&expr, &tags(&["other"])));
    }

    #[test]
    fn composite_expression() {
        // has(read) and not has(deprecated)
        let expr = TagExpr::And(
            Box::new(TagExpr::Has(tag("read"))),
            Box::new(TagExpr::Not(Box::new(TagExpr::Has(tag("deprecated"))))),
        );
        assert!(eval(&expr, &tags(&["read"])));
        assert!(!eval(&expr, &tags(&["read", "deprecated"])));
    }
}
