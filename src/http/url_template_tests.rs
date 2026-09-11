//! The I3 attack table (asserted against `render`'s output) plus a template-parse rejection test
//! for every rule in `UrlTemplate::parse`'s doc list. Split out from `url_template.rs` to keep
//! that file under the 400-line cap.

use std::collections::BTreeMap;

use super::*;

fn single_placeholder_template() -> UrlTemplate {
    UrlTemplate::parse("/{x}").expect("valid template")
}

fn render_one(value: &str) -> Result<String, TemplateError> {
    let mut values = BTreeMap::new();
    values.insert("x".to_owned(), value.to_owned());
    single_placeholder_template().render(&values)
}

#[test]
fn attack_table() {
    // Each expected value is the encoded *segment* per the plan's attack table; `render_one`
    // renders a full path (`/{x}` is the template), so it always has a leading `/` this table
    // doesn't spell out — the comparison below adds it back rather than baking it into the table.
    let cases: &[(&str, Result<&str, ()>)] = &[
        ("a/b", Ok("a%2Fb")),
        ("..", Err(())),
        (".", Err(())),
        ("a/../b", Ok("a%2F..%2Fb")),
        ("%2F", Ok("%252F")),
        ("", Err(())),
        ("http://evil/", Ok("http%3A%2F%2Fevil%2F")),
        ("a@b", Ok("a%40b")),
        ("?a=1", Ok("%3Fa%3D1")),
        ("#frag", Ok("%23frag")),
        ("a\nb", Err(())),
        ("é", Ok("%C3%A9")),
    ];

    for (input, expected) in cases {
        let got = render_one(input);
        match expected {
            Ok(want) => {
                let want_path = format!("/{want}");
                assert_eq!(
                    got.as_deref(),
                    Ok(want_path.as_str()),
                    "value {input:?} should render to segment {want:?}, got {got:?}"
                );
            }
            Err(()) => assert!(
                got.is_err(),
                "value {input:?} should be rejected, got {got:?}"
            ),
        }
    }
}

#[test]
fn rejects_template_not_starting_with_slash() {
    assert!(matches!(
        UrlTemplate::parse("users/{id}"),
        Err(TemplateError::NotAbsolute(_))
    ));
}

#[test]
fn rejects_template_starting_with_double_slash() {
    assert!(matches!(
        UrlTemplate::parse("//evil.example.com/x"),
        Err(TemplateError::DoubleSlashStart(_))
    ));
}

#[test]
fn rejects_query_in_template() {
    assert!(matches!(
        UrlTemplate::parse("/search?q={q}"),
        Err(TemplateError::ContainsQueryOrFragment(_))
    ));
}

#[test]
fn rejects_fragment_in_template() {
    assert!(matches!(
        UrlTemplate::parse("/page#{frag}"),
        Err(TemplateError::ContainsQueryOrFragment(_))
    ));
}

#[test]
fn rejects_scheme_separator_in_template() {
    assert!(matches!(
        UrlTemplate::parse("/redirect/http://evil"),
        Err(TemplateError::ContainsSchemeSeparator(_))
    ));
}

#[test]
fn rejects_mixed_literal_and_placeholder_prefix() {
    assert!(matches!(
        UrlTemplate::parse("/pre{x}"),
        Err(TemplateError::MixedSegment(_))
    ));
}

#[test]
fn rejects_two_placeholders_in_one_segment() {
    assert!(matches!(
        UrlTemplate::parse("/{a}{b}"),
        Err(TemplateError::MixedSegment(_))
    ));
}

#[test]
fn rejects_placeholder_with_trailing_literal() {
    assert!(matches!(
        UrlTemplate::parse("/{x}post"),
        Err(TemplateError::MixedSegment(_))
    ));
}

#[test]
fn rejects_empty_placeholder_name() {
    assert!(matches!(
        UrlTemplate::parse("/{}"),
        Err(TemplateError::MixedSegment(_))
    ));
}

#[test]
fn rejects_dot_segment() {
    assert!(matches!(
        UrlTemplate::parse("/a/./b"),
        Err(TemplateError::DotSegment(_))
    ));
}

#[test]
fn rejects_dot_dot_segment() {
    assert!(matches!(
        UrlTemplate::parse("/a/../b"),
        Err(TemplateError::DotSegment(_))
    ));
}

#[test]
fn rejects_empty_interior_segment() {
    assert!(matches!(
        UrlTemplate::parse("/a//b"),
        Err(TemplateError::EmptySegment)
    ));
}

#[test]
fn rejects_trailing_slash_as_an_empty_segment() {
    assert!(matches!(
        UrlTemplate::parse("/a/"),
        Err(TemplateError::EmptySegment)
    ));
}

#[test]
fn rejects_duplicate_placeholder() {
    assert!(matches!(
        UrlTemplate::parse("/{id}/nested/{id}"),
        Err(TemplateError::DuplicatePlaceholder(_))
    ));
}

#[test]
fn rejects_malformed_percent_encoding() {
    assert!(matches!(
        UrlTemplate::parse("/a%2gb"),
        Err(TemplateError::MalformedPercentEncoding(_))
    ));
}

#[test]
fn rejects_percent_at_end_of_segment() {
    assert!(matches!(
        UrlTemplate::parse("/a%2"),
        Err(TemplateError::MalformedPercentEncoding(_))
    ));
}

#[test]
fn accepts_well_formed_percent_encoding_in_a_literal() {
    assert!(UrlTemplate::parse("/a%2Fb").is_ok());
}

#[test]
fn missing_value_is_an_error() {
    let t = single_placeholder_template();
    assert!(matches!(
        t.render(&BTreeMap::new()),
        Err(TemplateError::MissingValue(_))
    ));
}

#[test]
fn never_percent_decodes_input() {
    // A caller sending the literal three characters '%', '2', 'F' must get '%2F' encoded again
    // as '%252F', never a literal '/' — decoding on the way in would let this value split into
    // two path segments.
    assert_eq!(render_one("%2F").expect("renders"), "/%252F");
}

#[test]
fn no_unicode_normalisation_exact_utf8_bytes_are_encoded() {
    assert_eq!(render_one("é").expect("renders"), "/%C3%A9");
}
