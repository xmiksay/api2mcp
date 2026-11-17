//! I3, structurally: a path template is parsed once, at definition time, into a fixed sequence
//! of segments where a placeholder is always a *whole* segment (`/pre{x}` and `/{a}{b}` are
//! parse errors, not accepted-then-sanitised). [`UrlTemplate::render`] then percent-encodes each
//! substitution with an unreserved-only set, so a value can never introduce a `/` and split into
//! a second segment.
//!
//! Two things this module deliberately does *not* do, because either one would reopen I3:
//! - **No percent-decoding of input.** A param value is a raw string; a caller sending the three
//!   literal characters `%2F` gets `%252F` in the rendered path, never a smuggled `/`.
//! - **No Unicode normalisation.** The exact UTF-8 bytes of the value are what gets encoded, so
//!   the bytes a validator inspected and the bytes the upstream server receives are identical —
//!   no NFC/NFD gap for a homoglyph or combining-character trick to hide in.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use thiserror::Error;

/// One element of a parsed template: fixed text, or a whole-segment placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    /// The placeholder name, without braces (e.g. `id` for `{id}`).
    Placeholder(String),
}

/// A path template compiled from a raw string like `/users/{id}/orders`. Construct with
/// [`UrlTemplate::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlTemplate {
    segments: Vec<Segment>,
}

/// Rejections from both [`UrlTemplate::parse`] (structural, about the template itself — a
/// definer-time error) and [`UrlTemplate::render`] (about one substitution value — a call-time
/// error). One enum because both are the same kind of I3 violation, "this input could move the
/// request off its declared shape," just caught at different times.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum TemplateError {
    #[error("template {template:?} must start with '/'")]
    NotAbsolute { template: String },
    #[error("template {template:?} must not start with '//'")]
    DoubleSlashStart { template: String },
    #[error("template {template:?} must not contain '?' or '#'")]
    ContainsQueryOrFragment { template: String },
    #[error("template {template:?} must not contain a scheme separator '://'")]
    ContainsSchemeSeparator { template: String },
    #[error(
        "segment {segment:?} mixes literal text with a placeholder, or is a malformed placeholder"
    )]
    MixedSegment { segment: String },
    #[error("segment {segment:?} is a '.' or '..' dot-segment")]
    DotSegment { segment: String },
    #[error("template contains an empty path segment")]
    EmptySegment,
    #[error("placeholder {placeholder:?} is declared more than once")]
    DuplicatePlaceholder { placeholder: String },
    #[error("literal segment {segment:?} contains a malformed percent-encoding")]
    MalformedPercentEncoding { segment: String },

    #[error("no value supplied for placeholder {placeholder:?}")]
    MissingValue { placeholder: String },
    #[error("value for placeholder {placeholder:?} is empty")]
    EmptyValue { placeholder: String },
    #[error("value for placeholder {placeholder:?} is exactly '.' or '..'")]
    DotValue { placeholder: String },
    #[error("value for placeholder {placeholder:?} contains a control character")]
    ControlCharacter { placeholder: String },
}

/// Unreserved characters per RFC 3986 §2.3 (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) are the only
/// bytes left un-encoded; everything else ASCII — crucially `/` — is encoded, which is the whole
/// mechanism that keeps a param value from ever introducing a second path segment. Non-ASCII
/// bytes are always encoded by `utf8_percent_encode` regardless of this set.
static UNRESERVED_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

impl UrlTemplate {
    /// Parses a raw path template into a fixed segment sequence. See the module docs for the
    /// rejection list.
    pub fn parse(template: &str) -> Result<UrlTemplate, TemplateError> {
        if !template.starts_with('/') {
            return Err(TemplateError::NotAbsolute {
                template: template.to_owned(),
            });
        }
        if template.starts_with("//") {
            return Err(TemplateError::DoubleSlashStart {
                template: template.to_owned(),
            });
        }
        if template.contains('?') || template.contains('#') {
            return Err(TemplateError::ContainsQueryOrFragment {
                template: template.to_owned(),
            });
        }
        if template.contains("://") {
            return Err(TemplateError::ContainsSchemeSeparator {
                template: template.to_owned(),
            });
        }
        if template == "/" {
            return Ok(UrlTemplate {
                segments: Vec::new(),
            });
        }

        let mut segments = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for raw in template[1..].split('/') {
            if raw.is_empty() {
                return Err(TemplateError::EmptySegment);
            }
            let segment = parse_segment(raw)?;
            if let Segment::Placeholder(name) = &segment
                && !seen.insert(name.clone())
            {
                return Err(TemplateError::DuplicatePlaceholder {
                    placeholder: name.clone(),
                });
            }
            segments.push(segment);
        }
        Ok(UrlTemplate { segments })
    }

    /// The template's parsed segments — used by `resolve::compile::assert_path_params_correspond`
    /// for the resolve-time cross-check that every placeholder here has a matching
    /// `location = Path` param, and vice versa.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Renders the template into a concrete path, always starting with `/`. `values` maps
    /// placeholder name to its raw, never-percent-decoded string value.
    pub fn render(&self, values: &BTreeMap<String, String>) -> Result<String, TemplateError> {
        if self.segments.is_empty() {
            return Ok("/".to_owned());
        }
        let mut out = String::new();
        for segment in &self.segments {
            out.push('/');
            match segment {
                Segment::Literal(literal) => out.push_str(literal),
                Segment::Placeholder(name) => {
                    let value = values
                        .get(name)
                        .ok_or_else(|| TemplateError::MissingValue {
                            placeholder: name.clone(),
                        })?;
                    validate_value(name, value)?;
                    out.extend(utf8_percent_encode(value, UNRESERVED_ENCODE_SET));
                }
            }
        }
        Ok(out)
    }
}

fn parse_segment(raw: &str) -> Result<Segment, TemplateError> {
    if raw.contains('{') || raw.contains('}') {
        let inner = raw.strip_prefix('{').and_then(|s| s.strip_suffix('}'));
        let name = match inner {
            Some(name) if !name.is_empty() && !name.contains(['{', '}']) => name,
            _ => {
                return Err(TemplateError::MixedSegment {
                    segment: raw.to_owned(),
                });
            }
        };
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(TemplateError::MixedSegment {
                segment: raw.to_owned(),
            });
        }
        return Ok(Segment::Placeholder(name.to_owned()));
    }
    if raw == "." || raw == ".." {
        return Err(TemplateError::DotSegment {
            segment: raw.to_owned(),
        });
    }
    validate_percent_encoding(raw)?;
    Ok(Segment::Literal(raw.to_owned()))
}

fn validate_percent_encoding(s: &str) -> Result<(), TemplateError> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let valid = bytes
                .get(i + 1..i + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit));
            if !valid {
                return Err(TemplateError::MalformedPercentEncoding {
                    segment: s.to_owned(),
                });
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Render-time checks on a substitution value. Only three things are rejected outright.
///
/// Everything else — `/`, `?`, `#`, `@`, `%`, and the rest of ASCII punctuation — is made safe by
/// percent-encoding in `render`, so it stays *data*. Rejecting `?` or `#` as "suspicious" would
/// buy no safety (both are in the encode set, and the containment post-condition re-checks the
/// assembled URL either way) while breaking legitimate values that happen to contain one.
///
/// `.` and `..` are the exception that must be rejected rather than encoded: they are made
/// entirely of unreserved characters, so encoding leaves them untouched and `Url::parse` would
/// then normalise the segment away.
fn validate_value(name: &str, value: &str) -> Result<(), TemplateError> {
    if value.is_empty() {
        return Err(TemplateError::EmptyValue {
            placeholder: name.to_owned(),
        });
    }
    if value == "." || value == ".." {
        return Err(TemplateError::DotValue {
            placeholder: name.to_owned(),
        });
    }
    if value.chars().any(is_control_char) {
        return Err(TemplateError::ControlCharacter {
            placeholder: name.to_owned(),
        });
    }
    Ok(())
}

/// C0 (`U+0000..=U+001F`, `U+007F`) and C1 (`U+0080..=U+009F`) controls.
fn is_control_char(c: char) -> bool {
    matches!(c as u32, 0x00..=0x1F | 0x7F | 0x80..=0x9F)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_and_placeholder_segments() {
        let t = UrlTemplate::parse("/users/{id}/orders").expect("valid");
        assert_eq!(
            t.segments(),
            &[
                Segment::Literal("users".to_owned()),
                Segment::Placeholder("id".to_owned()),
                Segment::Literal("orders".to_owned()),
            ]
        );
    }

    #[test]
    fn root_template_has_no_segments() {
        let t = UrlTemplate::parse("/").expect("valid");
        assert!(t.segments().is_empty());
        assert_eq!(t.render(&BTreeMap::new()).expect("renders"), "/");
    }

    #[test]
    fn renders_a_placeholder_in_place() {
        let t = UrlTemplate::parse("/users/{id}").expect("valid");
        let mut values = BTreeMap::new();
        values.insert("id".to_owned(), "42".to_owned());
        assert_eq!(t.render(&values).expect("renders"), "/users/42");
    }
}

#[cfg(test)]
#[path = "url_template_tests.rs"]
mod attack_table_tests;
