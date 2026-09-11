//! A validated identifier used everywhere a human names something: services, api_calls,
//! scripts, endpoints, tags. Keeping validation in the type (not scattered at each call site)
//! means a bad slug can never reach a URL path segment, a DB unique key, or a generated tool
//! name — it is rejected once, at construction.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// `[a-z0-9_-]{1,64}`. Lowercase-only and dash/underscore-only keeps slugs safe to use verbatim
/// as MCP tool name segments and URL path segments without further escaping.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SlugError {
    #[error("slug must not be empty")]
    Empty,
    #[error("slug must be at most 64 bytes, got {0}")]
    TooLong(usize),
    #[error("slug contains invalid character {0:?} (allowed: a-z, 0-9, '_', '-')")]
    InvalidChar(char),
}

impl Slug {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl FromStr for Slug {
    type Err = SlugError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(SlugError::Empty);
        }
        // Byte length, not char count: a slug is a path/identifier surface, so a
        // multi-byte-but-short string shouldn't quietly slip past the 64-byte DB column cap.
        if s.len() > 64 {
            return Err(SlugError::TooLong(s.len()));
        }
        if let Some(c) = s
            .chars()
            .find(|&c| !matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
        {
            return Err(SlugError::InvalidChar(c));
        }
        Ok(Slug(s.to_owned()))
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Slug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Slug {
    type Error = SlugError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Slug> for String {
    fn from(slug: Slug) -> Self {
        slug.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_valid_slug() {
        let s: Slug = "demo-service_1".parse().expect("valid slug");
        assert_eq!(s.as_str(), "demo-service_1");
        assert_eq!(s.to_string(), "demo-service_1");
    }

    #[test]
    fn rejects_uppercase() {
        assert_eq!("Demo".parse::<Slug>(), Err(SlugError::InvalidChar('D')));
    }

    #[test]
    fn rejects_spaces() {
        assert_eq!(
            "demo service".parse::<Slug>(),
            Err(SlugError::InvalidChar(' '))
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!("".parse::<Slug>(), Err(SlugError::Empty));
    }

    #[test]
    fn rejects_over_64_chars() {
        let too_long = "a".repeat(65);
        assert_eq!(too_long.parse::<Slug>(), Err(SlugError::TooLong(65)));
    }

    #[test]
    fn accepts_exactly_64_chars() {
        let ok = "a".repeat(64);
        assert!(ok.parse::<Slug>().is_ok());
    }

    #[test]
    fn orders_lexicographically_for_btreeset_use() {
        let a: Slug = "a".parse().expect("valid");
        let b: Slug = "b".parse().expect("valid");
        assert!(a < b);
    }
}
