//! Every ownership case compiles as written and rejects its marked mutation.
#[test]
fn ui() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
