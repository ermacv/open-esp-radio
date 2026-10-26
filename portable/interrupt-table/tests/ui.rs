#![cfg(not(target_os = "none"))]

#[test]
fn contexts_and_directions_are_checked_at_compile_time() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
