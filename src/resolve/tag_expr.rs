//! Parser and printer for the `endpoints.tag_expr` grammar.
//!
//! Promoted verbatim from `store::endpoint::tag_expr_parse` (a pre-C6 stopgap) per this
//! chunk's brief — see that module's original doc comment for why it lived there first.
//! `store::endpoint` calls into [`parse`]/[`to_string`] here rather than owning a second
//! parser for the same grammar: two implementations of one grammar drift apart the moment
//! either gains a feature the other doesn't.
//!
//! Grammar, lowest to highest precedence:
//!
//! ```text
//! expr  := or
//! or    := and ("or" and)*
//! and   := unary ("and" unary)*
//! unary := "not" unary | atom
//! atom  := "has" "(" ident ")" | "(" expr ")"
//! ident := [a-z0-9_-]+
//! ```
//!
//! [`to_string`] always fully parenthesizes its output, so [`parse`] never needs to
//! implement precedence-climbing to stay round-trip-safe — every binary/unary operand it
//! sees is already an unambiguous `atom`.

use serde::Serialize;
use thiserror::Error;

use crate::model::{Tag, TagExpr};

/// A `tag_expr` string that doesn't parse. Kept independent of [`crate::store::StoreError`]
/// so this module stays a plain grammar (no store dependency); `store::endpoint` maps this
/// into `StoreError::Malformed` at its one call site.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[error("endpoints.tag_expr {input:?}: {message}")]
pub struct TagExprError {
    input: String,
    message: String,
}

pub fn to_string(expr: &TagExpr) -> String {
    match expr {
        TagExpr::Has(tag) => format!("has({})", tag.0.as_str()),
        TagExpr::Not(inner) => format!("not ({})", to_string(inner)),
        TagExpr::And(a, b) => format!("({}) and ({})", to_string(a), to_string(b)),
        TagExpr::Or(a, b) => format!("({}) or ({})", to_string(a), to_string(b)),
    }
}

pub fn parse(input: &str) -> Result<TagExpr, TagExprError> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let expr = parse_or(input, &tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(malformed(
            input,
            "trailing tokens after a complete expression",
        ));
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Has,
    And,
    Or,
    Not,
    LParen,
    RParen,
    Ident(String),
}

fn tokenize(input: &str) -> Result<Vec<Token>, TagExprError> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => {
                let start = i;
                while i < bytes.len() {
                    let c2 = bytes[i] as char;
                    if c2.is_ascii_alphanumeric() || c2 == '_' || c2 == '-' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let word = &input[start..i];
                tokens.push(match word {
                    "has" => Token::Has,
                    "and" => Token::And,
                    "or" => Token::Or,
                    "not" => Token::Not,
                    other => Token::Ident(other.to_owned()),
                });
            }
            other => return Err(malformed(input, &format!("unexpected character {other:?}"))),
        }
    }
    Ok(tokens)
}

fn parse_or(input: &str, tokens: &[Token], pos: &mut usize) -> Result<TagExpr, TagExprError> {
    let mut lhs = parse_and(input, tokens, pos)?;
    while matches!(tokens.get(*pos), Some(Token::Or)) {
        *pos += 1;
        let rhs = parse_and(input, tokens, pos)?;
        lhs = TagExpr::Or(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_and(input: &str, tokens: &[Token], pos: &mut usize) -> Result<TagExpr, TagExprError> {
    let mut lhs = parse_unary(input, tokens, pos)?;
    while matches!(tokens.get(*pos), Some(Token::And)) {
        *pos += 1;
        let rhs = parse_unary(input, tokens, pos)?;
        lhs = TagExpr::And(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_unary(input: &str, tokens: &[Token], pos: &mut usize) -> Result<TagExpr, TagExprError> {
    if matches!(tokens.get(*pos), Some(Token::Not)) {
        *pos += 1;
        return Ok(TagExpr::Not(Box::new(parse_unary(input, tokens, pos)?)));
    }
    parse_atom(input, tokens, pos)
}

fn parse_atom(input: &str, tokens: &[Token], pos: &mut usize) -> Result<TagExpr, TagExprError> {
    match tokens.get(*pos) {
        Some(Token::Has) => {
            *pos += 1;
            expect(input, tokens, pos, &Token::LParen)?;
            let name = match tokens.get(*pos) {
                Some(Token::Ident(name)) => name.clone(),
                _ => {
                    return Err(malformed_at(
                        input,
                        *pos,
                        "expected a tag name inside has(...)",
                    ));
                }
            };
            *pos += 1;
            expect(input, tokens, pos, &Token::RParen)?;
            let slug = name
                .parse()
                .map_err(|e| malformed(input, &format!("tag name {name:?}: {e}")))?;
            Ok(TagExpr::Has(Tag(slug)))
        }
        Some(Token::LParen) => {
            *pos += 1;
            let inner = parse_or(input, tokens, pos)?;
            expect(input, tokens, pos, &Token::RParen)?;
            Ok(inner)
        }
        _ => Err(malformed_at(
            input,
            *pos,
            "expected 'has(...)' or a parenthesized expression",
        )),
    }
}

fn expect(
    input: &str,
    tokens: &[Token],
    pos: &mut usize,
    want: &Token,
) -> Result<(), TagExprError> {
    if tokens.get(*pos) == Some(want) {
        *pos += 1;
        Ok(())
    } else {
        Err(malformed_at(input, *pos, &format!("expected {want:?}")))
    }
}

fn malformed(input: &str, msg: &str) -> TagExprError {
    TagExprError {
        input: input.to_owned(),
        message: msg.to_owned(),
    }
}

fn malformed_at(input: &str, pos: usize, msg: &str) -> TagExprError {
    TagExprError {
        input: input.to_owned(),
        message: format!("at token {pos}: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_a_single_has() {
        let expr = TagExpr::Has(Tag("read".parse().unwrap()));
        let s = to_string(&expr);
        assert_eq!(parse(&s).unwrap(), expr);
    }

    #[test]
    fn roundtrips_and_not_or() {
        let expr = TagExpr::And(
            Box::new(TagExpr::Has(Tag("read".parse().unwrap()))),
            Box::new(TagExpr::Not(Box::new(TagExpr::Has(Tag("deprecated"
                .parse()
                .unwrap()))))),
        );
        let s = to_string(&expr);
        assert_eq!(parse(&s).unwrap(), expr);
    }

    #[test]
    fn parses_the_doc_comment_example_grammar() {
        let expr = parse("has(read) and not has(deprecated)").unwrap();
        let expected = TagExpr::And(
            Box::new(TagExpr::Has(Tag("read".parse().unwrap()))),
            Box::new(TagExpr::Not(Box::new(TagExpr::Has(Tag("deprecated"
                .parse()
                .unwrap()))))),
        );
        assert_eq!(expr, expected);
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse("has(read) has(write)").is_err());
    }

    #[test]
    fn rejects_unknown_character() {
        assert!(parse("has(read) & has(write)").is_err());
    }

    #[test]
    fn roundtrips_the_module_docs_grammar_shape() {
        // `service:gitlab` isn't a valid ident (':' isn't allowed), so the same *shape* --
        // AND of a has() with an OR-of-two-has() with a NOT-has() -- is expressed with
        // valid tag names instead.
        let expr = TagExpr::And(
            Box::new(TagExpr::And(
                Box::new(TagExpr::Has(Tag("service-gitlab".parse().unwrap()))),
                Box::new(TagExpr::Or(
                    Box::new(TagExpr::Has(Tag("domain-mr".parse().unwrap()))),
                    Box::new(TagExpr::Has(Tag("domain-pipeline".parse().unwrap()))),
                )),
            )),
            Box::new(TagExpr::Not(Box::new(TagExpr::Has(Tag("access-write"
                .parse()
                .unwrap()))))),
        );
        let s = to_string(&expr);
        assert_eq!(parse(&s).unwrap(), expr);
    }

    #[test]
    fn parses_nested_parens_and_precedence() {
        // `or` binds looser than `and`: `has(a) and (has(b) or has(c))` must NOT parse the
        // same as `(has(a) and has(b)) or has(c)`.
        let with_parens = parse("has(a) and (has(b) or has(c))").unwrap();
        let expected = TagExpr::And(
            Box::new(TagExpr::Has(Tag("a".parse().unwrap()))),
            Box::new(TagExpr::Or(
                Box::new(TagExpr::Has(Tag("b".parse().unwrap()))),
                Box::new(TagExpr::Has(Tag("c".parse().unwrap()))),
            )),
        );
        assert_eq!(with_parens, expected);

        let without_parens = parse("has(a) and has(b) or has(c)").unwrap();
        let left_assoc = TagExpr::Or(
            Box::new(TagExpr::And(
                Box::new(TagExpr::Has(Tag("a".parse().unwrap()))),
                Box::new(TagExpr::Has(Tag("b".parse().unwrap()))),
            )),
            Box::new(TagExpr::Has(Tag("c".parse().unwrap()))),
        );
        assert_eq!(without_parens, left_assoc);
        assert_ne!(with_parens, without_parens);
    }

    #[test]
    fn rejects_unbalanced_parens() {
        assert!(parse("(has(a) and has(b)").is_err());
        assert!(parse("has(a)) and has(b)").is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse("").is_err());
    }

    #[test]
    fn rejects_invalid_tag_name() {
        // Uppercase isn't a valid `Slug` character.
        assert!(parse("has(Invalid)").is_err());
    }
}
