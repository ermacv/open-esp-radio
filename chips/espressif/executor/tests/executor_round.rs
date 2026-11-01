#![cfg(not(target_os = "none"))]

#[test]
fn an_interrupt_executor_starts_only_the_task_set_its_entry_polls() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let scratch = tempfile::Builder::new()
        .prefix("executor-round-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("own executor round build directory");
    oer_test_ui_cases::run_chip(
        &root,
        scratch.path(),
        "riscv32imafc-unknown-none-elf",
        &[
            ("oer-espressif-executor", "chip"),
            ("oer-espressif-platform-port", "esp32s31"),
        ],
        &["chips/espressif/executor/tests/chip-ui/executor_round.rs"],
    );
}
