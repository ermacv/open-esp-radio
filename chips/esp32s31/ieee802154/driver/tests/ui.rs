#[test]
fn ui() {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../target/r4d");
    std::fs::create_dir_all(&output).expect("UI output directory");
    let scratch = tempfile::Builder::new()
        .prefix("driver-ui-")
        .tempdir_in(output.canonicalize().expect("absolute UI output directory"))
        .expect("own UI build directory");
    oer_test_ui_cases::run(
        env!("CARGO_MANIFEST_DIR"),
        scratch.path().to_str().expect("UTF-8 repository path"),
        &[],
    );
}
