//! Each `tests/ui` case compiles as written and fails with its marked errors
//! when `ui_reject` is set.

#[test]
fn ui() {
    oer_test_ui_cases::run(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_TARGET_TMPDIR"),
        &[("bluetooth-engine", cfg!(feature = "bluetooth-engine"))],
    );
}

#[test]
#[cfg(feature = "bluetooth-engine")]
fn dtm_profile_chip() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("HAL is below chips/esp32s31");
    let scratch = tempfile::Builder::new()
        .prefix("dtm-profile-chip-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("own chip UI build directory");
    oer_test_ui_cases::run_chip(
        root,
        scratch.path(),
        "riscv32imafc-unknown-none-elf",
        &[("oer-esp32s31-hal", "bluetooth-engine")],
        &[
            "chips/esp32s31/hal/tests/ui/bluetooth_dtm_profile_has_no_other_roles.rs",
            "chips/esp32s31/hal/tests/chip-ui/bluetooth_dtm_submit_requires_dtm_profile.rs",
        ],
    );
}
