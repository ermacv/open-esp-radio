//! Valid and rejected uses of the typed DMA and placement boundaries.
#![forbid(unsafe_code)]

#[test]
fn ui() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}
