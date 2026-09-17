//! `regex_is_match`/`regex_find`/`regex_captures`/`regex_replace`/`regex_replace_all`/
//! `regex_split` over the `regex` crate (1.13.x — linear-time, no backtracking, so an
//! attacker-shaped pattern still can't blow the wall-clock budget by itself; a pathological
//! *pattern* still costs compile time and a huge *input* still costs match time, which is why
//! [`crate::script::engine`]'s operation/wall-clock limits remain the real backstop).
//!
//! **Compiled patterns are cached within one run.** [`RegexCache`] is constructed once per
//! [`register`] call — i.e. once per [`crate::script::engine::build_engine`] call, i.e. once per
//! run — and every regex-facing closure below holds a clone of the same `Arc`, so a script that
//! calls the same pattern in a loop compiles it exactly once. The cache (and every compiled
//! pattern in it) is dropped when the engine is, at the end of the run.
//!
//! An invalid pattern is a clean catchable error: `regex::Error`'s own `Display` already names
//! the problem precisely, so it's surfaced verbatim rather than replaced with a generic message.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, NativeCallContext};

/// A pattern-string -> compiled-`Regex` cache, cheap to clone (an `Arc` around the real map) so
/// every registered closure can hold its own handle to the one cache for this run.
#[derive(Clone, Default)]
struct RegexCache(Arc<Mutex<HashMap<String, Arc<Regex>>>>);

impl RegexCache {
    /// Returns the cached compiled pattern if present, otherwise compiles, caches, and returns
    /// it. A poisoned mutex (only reachable if some other regex closure panicked while holding
    /// the lock, which none of them do) is recovered from rather than propagated as a second
    /// panic — this cache must never be the reason a script run dies uncleanly.
    fn get_or_compile(&self, pattern: &str) -> Result<Arc<Regex>, regex::Error> {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(re) = guard.get(pattern) {
            return Ok(Arc::clone(re));
        }
        let re = Arc::new(Regex::new(pattern)?);
        guard.insert(pattern.to_owned(), Arc::clone(&re));
        Ok(re)
    }
}

fn runtime_err(ctx: &NativeCallContext, msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.to_owned().into(),
        ctx.call_position(),
    ))
}

fn compiled(
    cache: &RegexCache,
    ctx: &NativeCallContext,
    pattern: &str,
) -> Result<Arc<Regex>, Box<EvalAltResult>> {
    cache
        .get_or_compile(pattern)
        .map_err(|e| runtime_err(ctx, &format!("regex {pattern:?}: {e}")))
}

fn captures_to_map(caps: &regex::Captures, re: &Regex) -> Map {
    let mut map = Map::new();
    for i in 0..caps.len() {
        if let Some(m) = caps.get(i) {
            map.insert(i.to_string().into(), Dynamic::from(m.as_str().to_owned()));
        }
    }
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            map.insert(name.into(), Dynamic::from(m.as_str().to_owned()));
        }
    }
    map
}

/// Registers the six `regex_*` functions, each sharing one clone of a freshly-created
/// [`RegexCache`] — see the module docs for why that cache is scoped to one call of this
/// function (one run).
pub fn register(engine: &mut Engine) {
    let cache = RegexCache::default();

    let c = cache.clone();
    engine.register_fn(
        "regex_is_match",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str|
              -> Result<bool, Box<EvalAltResult>> {
            Ok(compiled(&c, &ctx, pattern)?.is_match(text))
        },
    );

    let c = cache.clone();
    engine.register_fn(
        "regex_find",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str|
              -> Result<Dynamic, Box<EvalAltResult>> {
            let re = compiled(&c, &ctx, pattern)?;
            Ok(re
                .find(text)
                .map_or(Dynamic::UNIT, |m| Dynamic::from(m.as_str().to_owned())))
        },
    );

    let c = cache.clone();
    engine.register_fn(
        "regex_captures",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str|
              -> Result<Dynamic, Box<EvalAltResult>> {
            let re = compiled(&c, &ctx, pattern)?;
            Ok(re.captures(text).map_or(Dynamic::UNIT, |caps| {
                Dynamic::from(captures_to_map(&caps, &re))
            }))
        },
    );

    let c = cache.clone();
    engine.register_fn(
        "regex_replace",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str,
              replacement: &str|
              -> Result<String, Box<EvalAltResult>> {
            let re = compiled(&c, &ctx, pattern)?;
            Ok(re.replacen(text, 1, replacement).into_owned())
        },
    );

    let c = cache.clone();
    engine.register_fn(
        "regex_replace_all",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str,
              replacement: &str|
              -> Result<String, Box<EvalAltResult>> {
            let re = compiled(&c, &ctx, pattern)?;
            Ok(re.replace_all(text, replacement).into_owned())
        },
    );

    engine.register_fn(
        "regex_split",
        move |ctx: NativeCallContext,
              pattern: &str,
              text: &str|
              -> Result<Array, Box<EvalAltResult>> {
            let re = compiled(&cache, &ctx, pattern)?;
            Ok(re
                .split(text)
                .map(|s| Dynamic::from(s.to_owned()))
                .collect())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_compile_returns_the_same_compiled_pattern_on_repeat_calls() {
        let cache = RegexCache::default();
        let a = cache.get_or_compile(r"\d+").unwrap();
        let b = cache.get_or_compile(r"\d+").unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "second call must reuse the cached compile"
        );
    }

    #[test]
    fn get_or_compile_surfaces_an_invalid_pattern_by_name() {
        let cache = RegexCache::default();
        let err = cache.get_or_compile("(unclosed").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    fn engine() -> Engine {
        let mut e = Engine::new_raw();
        register(&mut e);
        e
    }

    #[test]
    fn is_match_and_find() {
        let e = engine();
        assert!(
            e.eval::<bool>(r#"regex_is_match("^\\d+$", "123")"#)
                .unwrap()
        );
        let found: String = e.eval(r#"regex_find("\\d+", "abc123def")"#).unwrap();
        assert_eq!(found, "123");
    }

    #[test]
    fn captures_include_numbered_and_named_groups() {
        let e = engine();
        let m: Map = e
            .eval(r#"regex_captures("(?P<y>\\d{4})-(?P<m>\\d{2})", "2024-03")"#)
            .unwrap();
        assert_eq!(
            m.get("0").unwrap().clone().into_string().unwrap(),
            "2024-03"
        );
        assert_eq!(m.get("y").unwrap().clone().into_string().unwrap(), "2024");
        assert_eq!(m.get("m").unwrap().clone().into_string().unwrap(), "03");
    }

    #[test]
    fn no_match_yields_unit_not_an_error() {
        let e = engine();
        let v: Dynamic = e.eval(r#"regex_find("zzz", "abc")"#).unwrap();
        assert!(v.is_unit());
    }

    #[test]
    fn replace_and_replace_all_and_split() {
        let e = engine();
        assert_eq!(
            e.eval::<String>(r#"regex_replace("a", "banana", "o")"#)
                .unwrap(),
            "bonana",
            "only the first match (index 1) is replaced"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace_all("a", "banana", "o")"#)
                .unwrap(),
            "bonono"
        );
        let parts: Array = e.eval(r#"regex_split(",", "a,b,c")"#).unwrap();
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn an_invalid_pattern_is_a_clean_catchable_error() {
        let e = engine();
        let err = e
            .eval::<bool>(r#"regex_is_match("(unclosed", "x")"#)
            .unwrap_err();
        assert!(matches!(*err, rhai::EvalAltResult::ErrorRuntime(..)));
    }
}
