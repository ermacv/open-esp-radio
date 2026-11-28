//! The PHY's ownership on the chip: each `tests/chip-ui` case compiles as
//! written and fails with its marked error when `ui_reject` is set.

#[test]
fn an_operation_is_lent_and_a_proof_is_branded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("the PHY is below chips/esp32s31/radio");
    let scratch = tempfile::Builder::new()
        .prefix("phy-ownership-chip-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("own chip UI build directory");
    oer_tools_test_ui_cases::run_chip(
        root,
        scratch.path(),
        "riscv32imafc-unknown-none-elf",
        &[("oer-chips-esp32s31-radio-phy", "radio-health")],
        &[
            "chips/esp32s31/radio/phy/tests/chip-ui/an_operation_cannot_keep_the_registers.rs",
            "chips/esp32s31/radio/phy/tests/chip-ui/a_proof_of_another_phy_serves_no_handle.rs",
            "chips/esp32s31/radio/phy/tests/chip-ui/a_cold_place_has_no_state_to_read.rs",
        ],
    );
}
