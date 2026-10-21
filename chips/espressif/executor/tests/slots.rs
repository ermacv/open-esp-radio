#![cfg(not(target_os = "none"))]

#[test]
fn static_slot_claims_and_backing_are_affine() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}

#[test]
fn static_slot_claims_and_backing_are_affine_on_rv32() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    oer_test_ui_cases::run_chip(
        &root,
        &std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("static-port-slots"),
        "riscv32imafc-unknown-none-elf",
        &[("oer-espressif-executor", "port-slots")],
        &["chips/espressif/executor/tests/ui/static_slot_duplicated.rs"],
    );
}
