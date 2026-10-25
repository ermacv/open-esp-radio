#![cfg(not(target_os = "none"))]
use std::path::Path;

#[test]
fn s31_service_owners_have_no_duplicate_or_unwitnessed_access() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    oer_test_ui_cases::run_chip(
        &root,
        &Path::new(env!("CARGO_TARGET_TMPDIR")).join("s31-service-ownership"),
        "riscv32imafc-unknown-none-elf",
        &[
            ("oer-espressif-executor", "esp32s31"),
            ("oer-espressif-esp-hal-soc", "esp32s31"),
        ],
        &["chips/espressif/executor/tests/chip/s31.rs"],
    );
}
