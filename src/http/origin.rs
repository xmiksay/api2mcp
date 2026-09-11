//! Origin allowlist checking (I2), built directly on [`crate::model::Origin`] — no second origin
//! type lives in this crate. Exact match only: an allowlist entry never matches by wildcard,
//! prefix, or subdomain, so adding an origin to a service always means an explicit, auditable
//! row rather than a pattern someone has to reason about.

use std::collections::BTreeSet;

use crate::model::Origin;

use super::GuardError;

/// Errors unless `origin` is exactly present in `allowlist`.
pub fn assert_allowed(origin: &Origin, allowlist: &BTreeSet<Origin>) -> Result<(), GuardError> {
    if allowlist.contains(origin) {
        Ok(())
    } else {
        Err(GuardError::OriginNotAllowed {
            origin: origin.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(s: &str) -> Origin {
        s.parse().expect("valid origin")
    }

    #[test]
    fn allows_exact_member() {
        let allowlist = BTreeSet::from([origin("https://api.example.com")]);
        assert!(assert_allowed(&origin("https://api.example.com"), &allowlist).is_ok());
    }

    #[test]
    fn rejects_non_member() {
        let allowlist = BTreeSet::from([origin("https://api.example.com")]);
        let err = assert_allowed(&origin("https://evil.example.com"), &allowlist).unwrap_err();
        assert_eq!(
            err,
            GuardError::OriginNotAllowed {
                origin: "https://evil.example.com".to_owned()
            }
        );
    }

    #[test]
    fn rejects_subdomain_as_not_a_wildcard_match() {
        let allowlist = BTreeSet::from([origin("https://example.com")]);
        assert!(assert_allowed(&origin("https://sub.example.com"), &allowlist).is_err());
    }

    #[test]
    fn scheme_mismatch_is_a_different_origin() {
        let allowlist = BTreeSet::from([origin("https://example.com")]);
        assert!(assert_allowed(&origin("http://example.com"), &allowlist).is_err());
    }

    #[test]
    fn port_mismatch_is_a_different_origin() {
        let allowlist = BTreeSet::from([origin("https://example.com:8443")]);
        assert!(assert_allowed(&origin("https://example.com"), &allowlist).is_err());
    }

    #[test]
    fn empty_allowlist_rejects_everything() {
        let allowlist = BTreeSet::new();
        assert!(assert_allowed(&origin("https://example.com"), &allowlist).is_err());
    }
}
