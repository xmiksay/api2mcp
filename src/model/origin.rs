//! A newtype over [`url::Origin`] that adds the total ordering `url::Origin` itself doesn't
//! provide. `Service::origin_allowlist` and `AuthProvider::bound_origin` need to live in a
//! `BTreeSet`/be compared, and I7 bans `Hash*` collections here, so a raw `url::Origin` (which
//! derives only `PartialEq + Eq + Hash`) can't be the field type without this wrapper.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin(url::Origin);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OriginError {
    #[error("not a valid URL: {0}")]
    InvalidUrl(String),
    #[error("origin is opaque (scheme has no host/port), not a reachable network origin")]
    Opaque,
}

impl Origin {
    /// The origin of `url` (scheme, host, port), rejecting opaque origins (e.g. `data:` URLs) —
    /// an opaque origin can never be a member of a reachable-origin set (I2).
    pub fn of(url: &url::Url) -> Result<Self, OriginError> {
        let origin = url.origin();
        if !origin.is_tuple() {
            return Err(OriginError::Opaque);
        }
        Ok(Origin(origin))
    }

    /// The canonical ASCII form (e.g. `https://example.com:8443`), used both for `Display` and
    /// as the sort key — origins are compared byte-wise, never by parsing them apart again.
    pub fn ascii_serialization(&self) -> String {
        self.0.ascii_serialization()
    }
}

impl FromStr for Origin {
    type Err = OriginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = url::Url::parse(s).map_err(|e| OriginError::InvalidUrl(e.to_string()))?;
        Origin::of(&url)
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.ascii_serialization())
    }
}

impl PartialOrd for Origin {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Origin {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.ascii_serialization().cmp(&other.ascii_serialization())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scheme_host_port() {
        let o: Origin = "https://example.com:8443/path?query"
            .parse()
            .expect("valid");
        assert_eq!(o.to_string(), "https://example.com:8443");
    }

    #[test]
    fn default_port_is_normalised_away() {
        let a: Origin = "https://example.com/".parse().expect("valid");
        let b: Origin = "https://example.com:443/".parse().expect("valid");
        assert_eq!(a, b);
    }

    #[test]
    fn different_scheme_is_a_different_origin() {
        let a: Origin = "http://example.com/".parse().expect("valid");
        let b: Origin = "https://example.com/".parse().expect("valid");
        assert_ne!(a, b);
    }

    #[test]
    fn orders_by_ascii_serialization() {
        let a: Origin = "https://a.example.com/".parse().expect("valid");
        let b: Origin = "https://b.example.com/".parse().expect("valid");
        assert!(a < b);
    }

    #[test]
    fn rejects_opaque_origin() {
        assert_eq!(
            "data:text/plain,hi".parse::<Origin>(),
            Err(OriginError::Opaque)
        );
    }
}
