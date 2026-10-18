//! Each `tests/ui` case compiles as written and fails with its marked errors
//! when `ui_reject` is set.

#[test]
fn ui() {
    oer_base_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
