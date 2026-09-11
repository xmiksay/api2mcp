//! A stopgap parser/printer for `endpoints.tag_expr` — see the module doc on
//! `super` (`store::endpoint`) for why this exists here instead of in `resolve::tag_expr`
//! (chunk C6).
//!
//! **For chunk C6**: promote this module verbatim into `resolve::tag_expr` rather than
//! writing a second parser for the same grammar next to it — two implementations of one
//! grammar are a bug waiting to happen the moment either one gains a feature the other
//! doesn't. If `resolve::tag_expr` needs something this parser doesn't do, extend this one
//! (or replace it outright) and have `store::endpoint` depend on the result, instead of
//! leaving both to drift.
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

use crate::model::{Tag, TagExpr};
use crate::store::StoreError;

pub(super) fn to_string(expr: &TagExpr) -> String {
    match expr {
        TagExpr::Has(tag) => format!("has({})", tag.0.as_str()),
        TagExpr::Not(inner) => format!("not ({})", to_string(inner)),
        TagExpr::And(a, b) => format!("({}) and ({})", to_string(a), to_string(b)),
        TagExpr::Or(a, b) => format!("({}) or ({})", to_string(a), to_string(b)),
    }
}

pub(super) fn parse(input: &str) -> Result<TagExpr, StoreError> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let expr = parse_or(&tokens, &mut pos)?;
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

fn tokenize(input: &str) -> Result<Vec<Token>, StoreError> {
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

fn parse_or(tokens: &[Token], pos: &mut usize) -> Result<TagExpr, StoreError> {
    let mut lhs = parse_and(tokens, pos)?;
    while matches!(tokens.get(*pos), Some(Token::Or)) {
        *pos += 1;
        let rhs = parse_and(tokens, pos)?;
        lhs = TagExpr::Or(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_and(tokens: &[Token], pos: &mut usize) -> Result<TagExpr, StoreError> {
    let mut lhs = parse_unary(tokens, pos)?;
    while matches!(tokens.get(*pos), Some(Token::And)) {
        *pos += 1;
        let rhs = parse_unary(tokens, pos)?;
        lhs = TagExpr::And(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_unary(tokens: &[Token], pos: &mut usize) -> Result<TagExpr, StoreError> {
    if matches!(tokens.get(*pos), Some(Token::Not)) {
        *pos += 1;
        return Ok(TagExpr::Not(Box::new(parse_unary(tokens, pos)?)));
    }
    parse_atom(tokens, pos)
}

fn parse_atom(tokens: &[Token], pos: &mut usize) -> Result<TagExpr, StoreError> {
    match tokens.get(*pos) {
        Some(Token::Has) => {
            *pos += 1;
            expect(tokens, pos, &Token::LParen)?;
            let name = match tokens.get(*pos) {
                Some(Token::Ident(name)) => name.clone(),
                _ => return Err(malformed_at(*pos, "expected a tag name inside has(...)")),
            };
            *pos += 1;
            expect(tokens, pos, &Token::RParen)?;
            let slug = name
                .parse()
                .map_err(|e| StoreError::Malformed(format!("tag name {name:?}: {e}")))?;
            Ok(TagExpr::Has(Tag(slug)))
        }
        Some(Token::LParen) => {
            *pos += 1;
            let inner = parse_or(tokens, pos)?;
            expect(tokens, pos, &Token::RParen)?;
            Ok(inner)
        }
        _ => Err(malformed_at(
            *pos,
            "expected 'has(...)' or a parenthesized expression",
        )),
    }
}

fn expect(tokens: &[Token], pos: &mut usize, want: &Token) -> Result<(), StoreError> {
    if tokens.get(*pos) == Some(want) {
        *pos += 1;
        Ok(())
    } else {
        Err(malformed_at(*pos, &format!("expected {want:?}")))
    }
}

fn malformed(input: &str, msg: &str) -> StoreError {
    StoreError::Malformed(format!("endpoints.tag_expr {input:?}: {msg}"))
}

fn malformed_at(pos: usize, msg: &str) -> StoreError {
    StoreError::Malformed(format!("endpoints.tag_expr: at token {pos}: {msg}"))
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
}
