#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::arithmetic_side_effects
)]
//! Each `tests/ui` case compiles as written and fails with its marked errors
//! when `ui_reject` is set.

#[test]
fn ui() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
