//! `trybuild` compile-fail cases for I4: a `Secret` must not be formattable or serializable.
//! See `src/secret/mod.rs` for what's deliberately missing and why.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
