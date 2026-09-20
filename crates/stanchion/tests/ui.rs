//! Compile-fail tests for `#[lua_class]` diagnostics.
//!
//! Run `TRYBUILD=overwrite cargo test --features lua54,vendored --test ui` to refresh
//! the expected output after changing a message.

#[test]
fn diagnostics() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
