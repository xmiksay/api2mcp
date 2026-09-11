//! The model-visible shape of anything that stops a script cold: a syntax error, an uncaught
//! runtime exception, a budget/timeout termination, or a panic escaping the blocking closure.
//! `thiserror`, never `anyhow` — this is the crate's own standard for anything that can reach a
//! tool-call caller, and a `String` context could smuggle a URL or a response fragment.
//!
//! `rhai::Position::line()`/`position()` are both `Option<usize>` — `None` means "no position
//! info at all" for `line()`, but for `position()` it *also* means "column 1" (beginning of
//! line). [`ScriptFailure::build`] disambiguates the two: a `None` column with a known line
//! becomes column 1, never a missing column.

use rhai::{EvalAltResult, ParseError, Position};
use serde::Serialize;
use thiserror::Error;

use crate::schema::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptFailureKind {
    /// The source failed to parse.
    Compile,
    /// An uncaught exception during evaluation: a script `throw`, a type error, an undeclared
    /// variable (`set_strict_variables`), an api_call name absent from I1's allowlist, ...
    Runtime,
    /// `on_progress` terminated the run, or the async bridge refused a batch because the run's
    /// wall-clock budget had already expired while the script was parked waiting for a reply
    /// (see `script::bridge`). Never catchable by the script's own `try`/`catch` — this is the
    /// one `EvalAltResult` variant `is_catchable()` returns `false` for.
    Terminated,
    /// The blocking closure panicked instead of returning. Converted here rather than allowed to
    /// take the process down with it.
    Panic,
}

/// A source line with a `^` caret under the offending column — present only when both a line
/// number and the original source text were available.
#[derive(Debug, Clone, Serialize, Error)]
#[error("{kind:?}: {message}")]
pub struct ScriptFailure {
    pub kind: ScriptFailureKind,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub snippet: Option<String>,
}

impl ScriptFailure {
    pub fn panic(message: impl Into<String>) -> Self {
        Self::bare(ScriptFailureKind::Panic, message)
    }

    pub fn terminated(message: impl Into<String>) -> Self {
        Self::bare(ScriptFailureKind::Terminated, message)
    }

    /// A runtime failure with no meaningful rhai position — e.g. binding a script's own declared
    /// parameters into scope, or marshalling its return value, both of which happen outside any
    /// single `rhai::Position`.
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::bare(ScriptFailureKind::Runtime, message)
    }

    fn bare(kind: ScriptFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            line: None,
            column: None,
            snippet: None,
        }
    }

    pub fn from_parse_error(source: &str, err: &ParseError) -> Self {
        let pos = err.1;
        Self::build(source, ScriptFailureKind::Compile, &err.0.to_string(), pos)
    }

    /// Any error rhai itself marks uncatchable (`EvalAltResult::is_catchable() == false` —
    /// `ErrorTerminated` from `on_progress`, but also `ErrorTooManyOperations` and its sibling
    /// resource-limit errors) round-trips as [`ScriptFailureKind::Terminated`] rather than
    /// [`ScriptFailureKind::Runtime`]: a script's own `try`/`catch` could not have swallowed any
    /// of these either, so the two Rust-side kinds track the same catchable/uncatchable line
    /// rhai already draws, rather than singling out one variant.
    pub fn from_eval_error(source: &str, err: &EvalAltResult) -> Self {
        let kind = if err.is_catchable() {
            ScriptFailureKind::Runtime
        } else {
            ScriptFailureKind::Terminated
        };
        let pos = err.position();
        let message = strip_position_suffix(&err.to_string(), pos);
        Self::build(source, kind, &message, pos)
    }

    fn build(source: &str, kind: ScriptFailureKind, message: &str, pos: Position) -> Self {
        let line = pos.line();
        let column = pos.position().or(line.map(|_| 1));
        let snippet = line.and_then(|l| snippet_with_caret(source, l, column.unwrap_or(1)));
        Self {
            kind,
            message: message.to_owned(),
            line,
            column,
            snippet,
        }
    }
}

/// rhai's own `Display` appends `" (line L, position P)"` whenever a position is known (see
/// `rhai::types::error::EvalAltResult::display`) — strip it so [`ScriptFailure::message`] doesn't
/// repeat what `line`/`column` already carry structurally.
fn strip_position_suffix(display: &str, pos: Position) -> String {
    if pos.is_none() {
        return display.to_owned();
    }
    let suffix = format!(" ({pos})");
    display.strip_suffix(&suffix).unwrap_or(display).to_owned()
}

fn snippet_with_caret(source: &str, line: usize, column: usize) -> Option<String> {
    let text = source.lines().nth(line.checked_sub(1)?)?;
    let caret_col = column.saturating_sub(1).min(text.chars().count());
    let caret = format!("{}^", " ".repeat(caret_col));
    Some(format!("{text}\n{caret}"))
}

/// What [`super::run_script`] hands back on failure: a caller-side argument mismatch (the script
/// was invoked with arguments its own declared params reject) is a different problem from the
/// script itself misbehaving, so it stays a distinct variant rather than getting folded into
/// [`ScriptFailure`].
#[derive(Debug, Clone, Serialize, Error)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum RunScriptError {
    #[error("arguments: {0}")]
    Args(#[from] ValidationError),
    #[error(transparent)]
    Script(#[from] ScriptFailure),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_places_the_caret_under_the_column() {
        let source = "let x = 1;\napi(\"bogus\", #{});\n";
        let pos = Position::new(2, 4);
        let failure = ScriptFailure::build(source, ScriptFailureKind::Runtime, "boom", pos);
        assert_eq!(failure.line, Some(2));
        assert_eq!(failure.column, Some(4));
        let snippet = failure.snippet.expect("snippet present");
        assert_eq!(snippet, "api(\"bogus\", #{});\n   ^");
    }

    #[test]
    fn beginning_of_line_position_becomes_column_one() {
        // `Position::position()` returns `None` for a beginning-of-line position, which must not
        // be confused with "no position info at all" (`line()` returning `None`).
        let pos = Position::new(3, 0);
        let failure = ScriptFailure::build("a\nb\nc\n", ScriptFailureKind::Compile, "x", pos);
        assert_eq!(failure.line, Some(3));
        assert_eq!(failure.column, Some(1));
    }

    #[test]
    fn no_position_at_all_yields_no_snippet() {
        let failure =
            ScriptFailure::build("a\nb\n", ScriptFailureKind::Runtime, "x", Position::NONE);
        assert_eq!(failure.line, None);
        assert_eq!(failure.column, None);
        assert_eq!(failure.snippet, None);
    }

    #[test]
    fn position_suffix_is_stripped_exactly_once() {
        let pos = Position::new(1, 5);
        let display = format!("Runtime error: boom ({pos})");
        assert_eq!(strip_position_suffix(&display, pos), "Runtime error: boom");
    }

    #[test]
    fn bare_constructors_carry_no_position() {
        let f = ScriptFailure::terminated("budget exceeded: wall_clock");
        assert_eq!(f.kind, ScriptFailureKind::Terminated);
        assert_eq!(f.line, None);
        let f = ScriptFailure::panic("join error");
        assert_eq!(f.kind, ScriptFailureKind::Panic);
    }
}
