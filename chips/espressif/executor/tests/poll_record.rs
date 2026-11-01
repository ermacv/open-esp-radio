#![cfg(not(target_os = "none"))]

// A start keeps the poll record of the task it starts; a direct start with
// another task's record must not compile, or the image checks would miss the
// started task's own poll root.
#[test]
fn a_start_takes_only_the_poll_record_of_its_own_task() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let scratch = tempfile::Builder::new()
        .prefix("poll-record-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("own poll record build directory");
    oer_test_ui_cases::run_chip(
        &root,
        scratch.path(),
        "riscv32imafc-unknown-none-elf",
        &[
            ("oer-espressif-executor", "chip"),
            ("oer-espressif-platform-port", "esp32s31"),
        ],
        &["chips/espressif/executor/tests/chip-ui/poll_record_of_another_task.rs"],
    );
}
