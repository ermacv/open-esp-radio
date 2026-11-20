//! Each `tests/ui` case compiles as written and fails with its marked errors
//! when `ui_reject` is set.

#[test]
fn ui() {
    oer_tools_test_ui_cases::run(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_TARGET_TMPDIR"),
        &[("test-support", cfg!(feature = "test-support"))],
    );
}

#[test]
fn psram_mapping_owns_its_single_loan() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("HAL is below chips/esp32s31");
    let scratch = tempfile::Builder::new()
        .prefix("psram-loan-chip-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("own chip UI build directory");
    oer_tools_test_ui_cases::run_chip(
        root,
        scratch.path(),
        "riscv32imafc-unknown-none-elf",
        &[("oer-chips-esp32s31-hal", "test-support")],
        &[
            "chips/esp32s31/hal/tests/chip-ui/psram_double_loan.rs",
            "chips/esp32s31/hal/tests/chip-ui/psram_mapping_cannot_be_forged.rs",
            "chips/esp32s31/hal/tests/chip-ui/psram_remap_after_loan.rs",
            "chips/esp32s31/hal/tests/chip-ui/psram_range_grants_no_loan.rs",
            "chips/esp32s31/hal/tests/chip-ui/psram_foreign_owner.rs",
        ],
    );
}
