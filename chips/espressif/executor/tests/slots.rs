#![cfg(not(target_os = "none"))]

#[test]
fn static_slot_claims_and_backing_are_affine() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
