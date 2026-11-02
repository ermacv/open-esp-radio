#[test]
fn runtime_requires_completed_adoption() {
    oer_tools_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
